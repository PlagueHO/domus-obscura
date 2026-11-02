use super::package::{Result, copy, files, notices, output, regular};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::process::Command;

const TARGET: &str = "x86_64-unknown-linux-gnu";
const ROOT: &str = "target/linux-package";
const ARCHIVE: &str = "target/domus-obscura-0.1.0-linux-x86_64.tar.gz";

pub fn require_host() -> Result<()> {
    if !cfg!(target_os = "linux") {
        return Err("Linux packaging requires native x86_64 GNU/Linux".into());
    }
    if output(Command::new("id").arg("-u"))?.trim() == "0" {
        return Err(
            "Linux package builds and application checks must run as a non-root user".into(),
        );
    }
    let host = output(Command::new("rustc").arg("-vV"))?;
    if !host.lines().any(|line| line == format!("host: {TARGET}")) {
        return Err("Linux packaging requires native x86_64 GNU Rust".into());
    }
    let os = fs::read_to_string("/etc/os-release")?;
    if !os.lines().any(|line| line == "ID=ubuntu")
        || !os.lines().any(|line| line == "VERSION_ID=\"24.04\"")
    {
        return Err("build the Linux candidate on the supported Ubuntu 24.04 baseline".into());
    }
    if output(Command::new("getconf").arg("GNU_LIBC_VERSION"))?.trim() != "glibc 2.39" {
        return Err("Linux packaging requires the glibc 2.39 build baseline".into());
    }
    Ok(())
}

fn imports(text: &str) -> Result<Vec<String>> {
    let mut dependencies = Vec::new();
    for line in text.lines().filter(|line| line.contains("(NEEDED)")) {
        let name = line
            .split_once('[')
            .and_then(|(_, rest)| rest.strip_suffix(']'))
            .ok_or("malformed ELF dependency")?;
        if !matches!(
            name,
            "libc.so.6"
                | "libm.so.6"
                | "libgcc_s.so.1"
                | "libpthread.so.0"
                | "libdl.so.2"
                | "librt.so.1"
                | "ld-linux-x86-64.so.2"
        ) {
            return Err(format!("unpackaged Linux runtime dependency: {name}").into());
        }
        dependencies.push(name.to_owned());
    }
    dependencies.sort();
    dependencies.dedup();
    if !dependencies.iter().any(|name| name == "libc.so.6") {
        return Err("ELF inspection did not find the GNU C runtime".into());
    }
    Ok(dependencies)
}

fn glibc_versions(text: &str) -> Result<Vec<String>> {
    let mut versions = Vec::new();
    for word in text.split_whitespace() {
        let Some(version) = word.strip_prefix("GLIBC_") else {
            continue;
        };
        let parts = version
            .split('.')
            .map(str::parse::<u32>)
            .collect::<std::result::Result<Vec<_>, _>>()?;
        if parts.len() < 2 || parts.len() > 3 || parts.as_slice() > [2, 39, 0].as_slice() {
            return Err(format!("ELF requires unsupported GLIBC_{version}").into());
        }
        versions.push(version.to_owned());
    }
    versions.sort();
    versions.dedup();
    if versions.is_empty() {
        return Err("ELF symbol version inspection returned no glibc requirements".into());
    }
    Ok(versions)
}

fn inspect(executable: &Path) -> Result<serde_json::Value> {
    fn readelf(option: &str, executable: &Path) -> Result<String> {
        output(
            Command::new("readelf")
                .env("LC_ALL", "C")
                .arg(option)
                .arg(executable),
        )
    }
    let header = readelf("--file-header", executable)?;
    if !header.contains("ELF64")
        || !header.contains("Advanced Micro Devices X86-64")
        || !header.contains("2's complement, little endian")
    {
        return Err("Linux payload is not a little-endian x86_64 ELF executable".into());
    }
    let program = readelf("--program-headers", executable)?;
    if !program.contains("[Requesting program interpreter: /lib64/ld-linux-x86-64.so.2]") {
        return Err("unsupported Linux ELF interpreter".into());
    }
    let dynamic = readelf("--dynamic", executable)?;
    if dynamic.contains("(RPATH)") || dynamic.contains("(RUNPATH)") {
        return Err("Linux payload must not use an external runtime search path".into());
    }
    let versions = readelf("--version-info", executable)?;
    Ok(serde_json::json!({
        "interpreter": "/lib64/ld-linux-x86-64.so.2",
        "dependencies": imports(&dynamic)?,
        "glibc_symbols": glibc_versions(&versions)?
    }))
}

fn modes(root: &Path) -> Result<BTreeMap<String, u32>> {
    files(root)?
        .keys()
        .map(|name| {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                Ok((
                    name.clone(),
                    fs::metadata(root.join(name))?.permissions().mode() & 0o7777,
                ))
            }
            #[cfg(not(unix))]
            {
                let _ = root;
                Ok((name.clone(), if name == "domus" { 0o755 } else { 0o644 }))
            }
        })
        .collect()
}

fn set_modes(root: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fn walk(root: &Path, directory: &Path) -> Result<()> {
            regular(directory)?;
            fs::set_permissions(directory, fs::Permissions::from_mode(0o755))?;
            for entry in fs::read_dir(directory)? {
                let path = entry?.path();
                regular(&path)?;
                if path.is_dir() {
                    walk(root, &path)?;
                } else {
                    fs::set_permissions(
                        &path,
                        fs::Permissions::from_mode(if path == root.join("domus") {
                            0o755
                        } else {
                            0o644
                        }),
                    )?;
                }
            }
            Ok(())
        }
        walk(root, root)?;
    }
    #[cfg(not(unix))]
    regular(root)?;
    Ok(())
}

fn verify(root: &Path) -> Result<BTreeMap<String, String>> {
    regular(root)?;
    let path = root.join("package-manifest.json");
    regular(&path)?;
    if fs::metadata(&path)?.len() > 2 * 1024 * 1024 {
        return Err("Linux manifest exceeds its size limit".into());
    }
    let manifest: serde_json::Value = serde_json::from_slice(&fs::read(path)?)?;
    if manifest["format"] != 1 || manifest["target"] != TARGET {
        return Err("unsupported Linux package manifest".into());
    }
    let observed = files(root)?;
    if manifest["files"] != serde_json::to_value(&observed)?
        || manifest["modes"] != serde_json::to_value(modes(root)?)?
    {
        return Err("Linux payload differs from its manifest; existing files are preserved".into());
    }
    for name in [
        "domus",
        "browser/index.html",
        "systemd/domus-obscura-api.service",
        "systemd/domus-obscura-worker.service",
        "linux-services.md",
        "notices/components.json",
        "package-check.py",
    ] {
        if !observed.contains_key(name) {
            return Err(format!("Linux payload is missing {name}").into());
        }
    }
    output(
        Command::new("python3")
            .arg("packaging/linux/package-check.py")
            .arg("--verify-installed")
            .arg(std::path::absolute(root)?),
    )?;
    if inspect(&root.join("domus"))? != manifest["elf"] {
        return Err("Linux ELF evidence differs from its manifest".into());
    }
    output(Command::new(root.join("domus")).arg("version"))?;
    Ok(observed)
}

fn archive_check(archive: &Path) -> Result<()> {
    output(
        Command::new("python3")
            .arg("packaging/linux/package-check.py")
            .arg(std::path::absolute(archive)?),
    )?;
    Ok(())
}

fn release_command(cargo: &std::ffi::OsStr) -> Command {
    let mut command = Command::new(cargo);
    command.args([
        "build",
        "--locked",
        "--release",
        "--package",
        "domus",
        "--target",
        TARGET,
        "--target-dir",
        "target",
    ]);
    command
}

pub fn check() -> Result<()> {
    verify(Path::new(ROOT))?;
    archive_check(Path::new(ARCHIVE))?;
    println!(
        "Linux archive and isolated lifecycle checks passed. Installed services and reboot remain separate checks."
    );
    Ok(())
}

pub fn build() -> Result<()> {
    require_host()?;
    if std::env::var_os("RUSTFLAGS").is_some()
        || std::env::var_os("CARGO_ENCODED_RUSTFLAGS").is_some()
    {
        return Err("Linux package builds require unset Rust link flag overrides".into());
    }
    let mut command = release_command(&std::env::var_os("CARGO").ok_or("CARGO is missing")?);
    super::native_jpeg::configure(&mut command)?;
    if !command.status()?.success() {
        return Err("Linux release compilation failed".into());
    }
    let staging = Path::new("target").join(format!("linux-package-{}.pending", std::process::id()));
    fs::create_dir(&staging)?;
    copy(
        &Path::new("target").join(TARGET).join("release/domus"),
        &staging.join("domus"),
    )?;
    copy(Path::new("frontend/dist-app"), &staging.join("browser"))?;
    copy(
        Path::new("crates/domus/migrations"),
        &staging.join("migrations"),
    )?;
    copy(
        Path::new("packaging/linux/systemd"),
        &staging.join("systemd"),
    )?;
    copy(
        Path::new("packaging/linux/package-check.py"),
        &staging.join("package-check.py"),
    )?;
    copy(
        Path::new("docs/linux-services.md"),
        &staging.join("linux-services.md"),
    )?;
    for name in [
        "Cargo.lock",
        "pnpm-lock.yaml",
        "rust-toolchain.toml",
        ".node-version",
        "package.json",
        "xtask/native-jpeg.cmake",
    ] {
        copy(
            Path::new(name),
            &staging.join("build-provenance").join(name),
        )?;
    }
    notices(&staging.join("notices"), TARGET, None)?;
    set_modes(&staging)?;
    let manifest = serde_json::json!({
        "format": 1, "target": TARGET, "version": env!("CARGO_PKG_VERSION"),
        "baseline": {"distribution": "Ubuntu 24.04", "glibc": "2.39", "systemd": "255"},
        "revision": output(Command::new("git").args(["rev-parse", "HEAD"]))?.trim(),
        "dirty": !output(Command::new("git").args(["status", "--porcelain"]))?.is_empty(),
        "rust": output(Command::new("rustc").arg("--version"))?.trim(),
        "node": output(Command::new("node").arg("--version"))?.trim(),
        "elf": inspect(&staging.join("domus"))?,
        "notice_review": "Collected exact source notices; independent distribution review remains #26.",
        "files": files(&staging)?, "modes": modes(&staging)?
    });
    fs::write(
        staging.join("package-manifest.json"),
        serde_json::to_vec_pretty(&manifest)?,
    )?;
    set_modes(&staging)?;
    verify(&staging)?;
    let archive = Path::new("target").join(format!(
        "linux-archive-{}.pending.tar.gz",
        std::process::id()
    ));
    if archive.exists() {
        return Err("pending Linux archive already exists; preserve it for inspection".into());
    }
    output(
        Command::new("tar")
            .args([
                "--format=ustar",
                "--sort=name",
                "--owner=0",
                "--group=0",
                "--numeric-owner",
                "-czf",
            ])
            .arg(&archive)
            .arg("-C")
            .arg(&staging)
            .arg("."),
    )?;
    archive_check(&archive)?;
    let previous = Path::new(ROOT);
    let previous_files = if previous.exists() {
        Some(verify(previous)?)
    } else {
        None
    };
    let final_archive = Path::new(ARCHIVE);
    let checksum = final_archive.with_extension("gz.sha256");
    if final_archive.exists() || checksum.exists() {
        regular(final_archive)?;
        regular(&checksum)?;
        if fs::read_to_string(&checksum)?.trim()
            != format!("{:x}", Sha256::digest(fs::read(final_archive)?))
        {
            return Err(
                "previous Linux archive changed or lacks its checksum; it was preserved".into(),
            );
        }
    }
    if let Some(previous_files) = previous_files {
        super::package::clear_verified(previous, previous_files)?;
    }
    fs::rename(staging, previous)?;
    fs::rename(&archive, final_archive)?;
    fs::write(
        checksum,
        format!("{:x}\n", Sha256::digest(fs::read(final_archive)?)),
    )?;
    println!(
        "Verified Linux candidate: {ARCHIVE}. Manual installation requires separate approval."
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linux_package_pins_the_release_output_directory() {
        let command = release_command(std::ffi::OsStr::new("cargo"));
        let arguments = command.get_args().collect::<Vec<_>>();
        assert!(arguments.windows(2).any(|pair| {
            pair == [
                std::ffi::OsStr::new("--target-dir"),
                std::ffi::OsStr::new("target"),
            ]
        }));
    }

    #[test]
    fn linux_package_rejects_unbundled_dependencies() {
        assert_eq!(
            imports("0x (NEEDED) Shared library: [libc.so.6]").ok(),
            Some(vec!["libc.so.6".into()])
        );
        for name in [
            "libturbojpeg.so.0",
            "libssl.so.3",
            "libsqlite3.so.0",
            "../libc.so.6",
        ] {
            assert!(imports(&format!("(NEEDED) [{name}]")).is_err());
        }
        assert!(imports("").is_err());
    }

    #[test]
    fn linux_package_enforces_glibc_baseline() {
        assert!(glibc_versions("Name: GLIBC_2.2.5 Name: GLIBC_2.39").is_ok());
        for version in [
            "GLIBC_2.40",
            "GLIBC_3.0",
            "GLIBC_PRIVATE",
            "GLIBC_2.39.1",
            "",
        ] {
            assert!(glibc_versions(version).is_err());
        }
    }
}
