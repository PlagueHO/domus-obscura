# Pinned SQLite build wrapper

Source: `libsqlite3-sys` 0.38.2 from crates.io.
Archive SHA-256:
`f1d20bef17f513b9b3004532233187769cd072d790971f4e4da0e346eb6401e8`.
The archive includes SQLite 3.53.2. The build downloads it from
`https://static.crates.io/crates/libsqlite3-sys/libsqlite3-sys-0.38.2.crate`.
`source.cmake` checks the archive SHA-256 before each extraction. A failed
download or checksum stops the build. There is no system-library fallback.
Only the required engine, header, Rust sources and MIT license are extracted.
SQLCipher and alternative-platform sources are not extracted or compiled.

Upstream source and generated files stay in the ignored `target/ns` cache
and Cargo output directories, not Git. The first build needs HTTPS access.
After a successful build, `CARGO_NET_OFFLINE=true` requires the cached archive.
A missing or changed archive is refused without replacing it.
The cache lock covers extraction, patching and compilation across build owners.

`sqlite3.patch` replaces the Unix open, close,
stat/lstat, access and unlink macros, and the Windows wide-character create,
close, delete, mapping-create and attribute macros. Native file methods and locking remain
upstream. Unscoped upstream VFS users keep their native behavior. Synthetic
Domus names have no native pathname authority and never fall back to path opens.
The historical experiments in `scripts/` are not used at runtime.

The build checks that the patched engine is byte-identical to the accepted
production engine (SHA-256
`5cfdd2a2f3ed399b84f5d848c256727d66c3711e20bca56cd6ae5f24456edfeb`).
The generated Rust bindings and error implementation come from the same
verified archive. Only its two crate attributes move to the local crate root.
The wrapper keeps the upstream Rust 2021 edition for those generated bindings.
The compiler options retain the repository's existing bundled configuration,
including static MSVC CRT selection for Windows packages. This wrapper supports
the application's native Windows/Linux build, not arbitrary upstream features.

The upstream MIT license is in `LICENSE`. SQLite's public-domain notice is in
`NOTICE-SQLITE` and remains in the extracted engine and header.
`native/adapter.c` and `native/native.h` are local application code under the
repository license. The Rust ownership boundary is `crates/native-sqlite`.
