# Pinned native namespace boundary

Source: `libsqlite3-sys` 0.38.2 from crates.io.
Archive SHA-256:
`f1d20bef17f513b9b3004532233187769cd072d790971f4e4da0e346eb6401e8`.
The archive includes SQLite 3.53.2. The upstream MIT license is in `LICENSE`.
SQLite's public-domain notice remains at the start of its original source files.
The archive's source files and original notices are retained.

Local changes compile `domus/adapter.c` and replace the Unix open, close,
stat/lstat, access and unlink macros, and the Windows wide-character create,
close, delete, mapping-create and attribute macros. Native file methods and locking remain
upstream. Unscoped upstream VFS users keep their native behavior. Synthetic
Domus names have no native pathname authority and never fall back to path opens.
The historical experiments in `scripts/` are not used at runtime.

`domus/adapter.c` and `domus/native.h` are local application code under the
repository license. The Rust ownership boundary is `crates/native-sqlite`.
