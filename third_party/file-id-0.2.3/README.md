# File Id

Domus Obscura vendors version 0.2.3 from the official crates.io archive.
Archive SHA-256: `e1fc6a637b6dc58414714eddd9170ff187ecb0933d4c7024d1abbd23a3cc26e9`.
Upstream revision: `a1d7c2d8f80786679d58ec6d5986a1d4278bc8cf`.

The local patch adds `get_high_res_file_id_from_file(&std::fs::File)`.
It exposes the existing Windows high-resolution query through a safe borrowed-file API.
The path-based high-resolution helper delegates to it.
No query, fallback or resource-lifetime logic changes.
Keep both upstream license files with this source and binary distributions.

[![» Docs](https://flat.badgen.net/badge/api/docs.rs/df3600)][docs]

A utility to read file IDs.

Modern file systems assign a unique ID to each file. On Linux and MacOS it is called an `inode number`, on Windows it is called `file index`.
Together with the `device id`, a file can be identified uniquely on a device at a given time.

Keep in mind though, that IDs may be re-used at some point.

## Example

```rust
let file_id = file_id::get_file_id(path).unwrap();

println!("{file_id:?}");
```

## Features

- `serde` for serde support, off by default

[docs]: https://docs.rs/file-id
