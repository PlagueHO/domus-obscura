use sha2::{Digest, Sha256};
use std::env;
use std::error::Error;
use std::fs::{self, OpenOptions};
use std::path::PathBuf;
use std::process::Command;

fn main() -> Result<(), Box<dyn Error>> {
    if cfg!(any(
        feature = "bundled-sqlcipher-vendored-openssl",
        feature = "loadable_extension"
    )) {
        return Err("the Domus SQLite boundary requires the bundled non-SQLCipher engine".into());
    }
    let target_os = env::var("CARGO_CFG_TARGET_OS")?;
    if !matches!(target_os.as_str(), "windows" | "linux") {
        return Err("the Domus SQLite boundary requires Windows or Linux".into());
    }
    let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").ok_or("missing manifest")?);
    let output = PathBuf::from(env::var_os("OUT_DIR").ok_or("missing build output")?);
    let workspace = manifest
        .parent()
        .and_then(|path| path.parent())
        .ok_or("missing workspace")?;
    // Keep compiler input paths short on Windows. The lock covers extraction,
    // patching and compilation across profiles and concurrent Cargo owners.
    let cache = workspace.join("target/ns");
    fs::create_dir_all(&cache)?;
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(cache.join("source.lock"))?;
    lock.lock()?;
    println!("cargo:rerun-if-env-changed=CARGO_NET_OFFLINE");
    let offline = matches!(env::var("CARGO_NET_OFFLINE").as_deref(), Ok("true" | "1"));
    let status = Command::new("cmake")
        .arg(format!("-DSOURCE_CACHE={}", cache.display()))
        .arg(format!("-DOFFLINE={}", if offline { "ON" } else { "OFF" }))
        .arg("-P")
        .arg(manifest.join("source.cmake"))
        .status()?;
    if !status.success() {
        return Err(format!("pinned SQLite source preparation failed: {status}").into());
    }
    let upstream = cache.join("libsqlite3-sys-0.38.2");
    let original = fs::read_to_string(upstream.join("sqlite3/sqlite3.c"))?;
    let patch = diffy::Patch::from_str(include_str!("sqlite3.patch"))?;
    let engine = diffy::apply(&original, &patch)?;
    // Preserve the exact engine from the accepted vendored implementation.
    if format!("{:x}", Sha256::digest(engine.as_bytes()))
        != "5cfdd2a2f3ed399b84f5d848c256727d66c3711e20bca56cd6ae5f24456edfeb"
    {
        return Err("patched SQLite source differs from the approved engine".into());
    }
    let engine_path = cache.join("sqlite3.c");
    fs::write(&engine_path, engine)?;
    let rust = fs::read_to_string(upstream.join("src/lib.rs"))?;
    let rust = rust
        .strip_prefix(
            "#![expect(non_snake_case, non_camel_case_types)]\n#![cfg_attr(not(test), no_std)]\n",
        )
        .ok_or("upstream SQLite Rust crate attributes changed")?;
    fs::write(output.join("upstream.rs"), rust)?;
    fs::copy(upstream.join("src/error.rs"), output.join("error.rs"))?;
    fs::copy(
        upstream.join("sqlite3/bindgen_bundled_version.rs"),
        output.join("bindgen.rs"),
    )?;
    let mut compiler = cc::Build::new();
    compiler
        .file(manifest.join("native/adapter.c"))
        .file(engine_path)
        .include(upstream.join("sqlite3"))
        .include(manifest.join("native"))
        .define("DOMUS_SQLITE_SOURCE_ADAPTER", None)
        .warnings(false);
    for flag in [
        "SQLITE_CORE",
        "SQLITE_DEFAULT_FOREIGN_KEYS=1",
        "SQLITE_ENABLE_API_ARMOR",
        "SQLITE_ENABLE_COLUMN_METADATA",
        "SQLITE_ENABLE_DBSTAT_VTAB",
        "SQLITE_ENABLE_FTS3",
        "SQLITE_ENABLE_FTS3_PARENTHESIS",
        "SQLITE_ENABLE_FTS5",
        "SQLITE_ENABLE_JSON1",
        "SQLITE_ENABLE_LOAD_EXTENSION=1",
        "SQLITE_ENABLE_MEMORY_MANAGEMENT",
        "SQLITE_ENABLE_RTREE",
        "SQLITE_ENABLE_STAT4",
        "SQLITE_SOUNDEX",
        "SQLITE_THREADSAFE=1",
        "SQLITE_USE_URI",
        "HAVE_USLEEP=1",
        "HAVE_ISNAN",
        "_POSIX_THREAD_SAFE_FUNCTIONS",
    ] {
        compiler.flag(format!("-D{flag}"));
    }
    if target_os == "windows" {
        println!("cargo:rustc-link-lib=ntdll");
        if env::var("CARGO_CFG_TARGET_ENV")? == "msvc"
            && env::var("CARGO_CFG_TARGET_FEATURE")?
                .split(',')
                .any(|feature| feature == "crt-static")
        {
            compiler.static_crt(true);
        }
    } else {
        compiler.flag("-DHAVE_LOCALTIME_R");
    }
    for name in [
        "SQLITE_MAX_VARIABLE_NUMBER",
        "SQLITE_MAX_EXPR_DEPTH",
        "SQLITE_MAX_COLUMN",
    ] {
        println!("cargo:rerun-if-env-changed={name}");
        if let Ok(value) = env::var(name) {
            compiler.flag(format!("-D{name}={value}"));
        }
    }
    println!("cargo:rerun-if-env-changed=LIBSQLITE3_FLAGS");
    if let Ok(flags) = env::var("LIBSQLITE3_FLAGS") {
        for flag in flags.split_whitespace() {
            if flag.starts_with("-D") || flag.starts_with("-U") {
                compiler.flag(flag);
            } else if flag.starts_with("SQLITE_") {
                compiler.flag(format!("-D{flag}"));
            } else {
                return Err(format!("unsupported LIBSQLITE3_FLAGS value: {flag}").into());
            }
        }
    }
    compiler.compile("sqlite3");
    println!("cargo:include={}", upstream.join("sqlite3").display());
    println!("cargo:lib_dir={}", output.display());
    for path in [
        "build.rs",
        "source.cmake",
        "sqlite3.patch",
        "native/adapter.c",
        "native/native.h",
    ] {
        println!("cargo:rerun-if-changed={path}");
    }
    println!(
        "cargo:rerun-if-changed={}",
        cache.join("libsqlite3-sys-0.38.2.crate").display()
    );
    Ok(())
}
