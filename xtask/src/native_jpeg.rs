use std::error::Error;
use std::path::PathBuf;
use std::process::Command;
use std::sync::OnceLock;

static LIBRARY: OnceLock<Result<PathBuf, String>> = OnceLock::new();
static STATIC_LIBRARY: OnceLock<Result<PathBuf, String>> = OnceLock::new();

/// Keep native compiler probes out of Cargo's deeply nested build directory.
/// The pinned source archive is checked by CMake before its first extraction.
pub fn configure(command: &mut Command) -> Result<(), Box<dyn Error>> {
    configure_with(command, false)
}

pub fn configure_static(command: &mut Command) -> Result<(), Box<dyn Error>> {
    configure_with(command, true)
}

fn configure_with(command: &mut Command, static_crt: bool) -> Result<(), Box<dyn Error>> {
    let library = if static_crt {
        &STATIC_LIBRARY
    } else {
        &LIBRARY
    };
    let library = library.get_or_init(|| build(static_crt).map_err(|error| error.to_string()));
    let path = library.as_ref().map_err(|error| error.to_owned())?;
    for (name, _) in std::env::vars_os() {
        let upper = name.to_string_lossy().to_ascii_uppercase();
        if upper.starts_with("TURBOJPEG_") || upper.contains("_TURBOJPEG_") {
            command.env_remove(name);
        }
    }
    command
        .env("TURBOJPEG_SOURCE", "explicit")
        .env("TURBOJPEG_STATIC", "1")
        .env("TURBOJPEG_BINDING", "pregenerated")
        .env("TURBOJPEG_LIB_DIR", path);
    Ok(())
}

fn build(static_crt: bool) -> Result<PathBuf, Box<dyn Error>> {
    let output = Command::new("rustc").arg("-vV").output()?;
    if !output.status.success() {
        return Err("could not identify the Rust host for native JPEG compilation".into());
    }
    let version = String::from_utf8(output.stdout)?;
    let host = version
        .lines()
        .find_map(|line| line.strip_prefix("host: "))
        .filter(|host| {
            host.bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        })
        .ok_or("Rust did not report a valid host triple")?;
    if std::env::var_os("CARGO_BUILD_TARGET").is_some_and(|target| target != host) {
        return Err("native JPEG tasks do not support cross-compilation".into());
    }
    let root = std::env::current_dir()?.join("target");
    let status = Command::new("cmake")
        .arg(format!("-DOUT_ROOT={}", root.display()))
        .arg(format!("-DHOST_KEY={host}"))
        .arg(format!(
            "-DSTATIC_CRT={}",
            if static_crt { "ON" } else { "OFF" }
        ))
        .args(["-P", "xtask/native-jpeg.cmake"])
        .status()?;
    if !status.success() {
        return Err(format!("pinned native JPEG build failed with {status}").into());
    }
    let path = PathBuf::from(
        std::fs::read_to_string(root.join("native").join(host).join(if static_crt {
            "library-path-static"
        } else {
            "library-path"
        }))?
        .trim(),
    );
    if !path.is_dir() {
        return Err("native JPEG build did not produce its library directory".into());
    }
    Ok(path)
}
