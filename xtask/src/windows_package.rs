use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

type Result<T> = std::result::Result<T, Box<dyn Error>>;
const TARGET: &str = "x86_64-pc-windows-msvc";
const MAX_FILES: usize = 8192;

fn output(command: &mut Command) -> Result<String> {
    let output = command.output()?;
    if !output.status.success() {
        return Err(format!(
            "build command failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    Ok(String::from_utf8(output.stdout)?)
}

fn regular(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return Err("package inputs must not be Windows reparse points".into());
        }
    }
    if metadata.is_symlink() {
        return Err("package inputs must not be links".into());
    }
    Ok(())
}

fn files(root: &Path) -> Result<BTreeMap<String, String>> {
    fn walk(
        root: &Path,
        directory: &Path,
        result: &mut BTreeMap<String, String>,
        depth: usize,
    ) -> Result<()> {
        regular(directory)?;
        if depth > 16 {
            return Err("package directory depth exceeds the limit".into());
        }
        for entry in fs::read_dir(directory)? {
            let path = entry?.path();
            regular(&path)?;
            if path.is_dir() {
                walk(root, &path, result, depth + 1)?;
            } else if path != root.join("package-manifest.json") {
                if result.len() >= MAX_FILES {
                    return Err("package file count exceeds the limit".into());
                }
                if fs::metadata(&path)?.len() > 256 * 1024 * 1024 {
                    return Err("package file exceeds the 256 MiB limit".into());
                }
                let name = path
                    .strip_prefix(root)?
                    .to_str()
                    .ok_or("package path is not UTF-8")?
                    .replace('\\', "/");
                result.insert(name, format!("{:x}", Sha256::digest(fs::read(path)?)));
            }
        }
        Ok(())
    }
    let mut result = BTreeMap::new();
    walk(root, root, &mut result, 0)?;
    Ok(result)
}

fn copy(source: &Path, target: &Path) -> Result<()> {
    regular(source)?;
    if source.is_dir() {
        fs::create_dir_all(target)?;
        for entry in fs::read_dir(source)? {
            let entry = entry?;
            copy(&entry.path(), &target.join(entry.file_name()))?;
        }
    } else {
        fs::create_dir_all(target.parent().ok_or("package file parent is missing")?)?;
        fs::copy(source, target)?;
    }
    Ok(())
}

fn notice_files(source: &Path, target: &Path) -> Result<usize> {
    let mut count = 0;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let name = entry.file_name();
        let upper = name.to_string_lossy().to_ascii_uppercase();
        if upper.contains("LICENSE")
            || upper.starts_with("LICENCE")
            || upper.starts_with("COPYING")
            || upper.starts_with("COPYRIGHT")
            || upper.starts_with("NOTICE")
            || upper == "README.IJG"
            || upper == "OFL.TXT"
        {
            copy(&entry.path(), &target.join(name))?;
            count += 1;
        }
    }
    Ok(count)
}

#[derive(serde::Deserialize)]
struct SupplementalNotice {
    name: String,
    version: String,
    repository: String,
    revision: String,
    source_directory: String,
    notice_directory: String,
    files: BTreeMap<String, String>,
}

fn supplemental_notices(
    package: &serde_json::Value,
    directory: &Path,
    destination: &Path,
) -> Result<usize> {
    let sources: Vec<SupplementalNotice> = serde_json::from_str(include_str!(
        "../../third_party/windows-distribution-dependencies/sources.json"
    ))?;
    let Some(source) = sources
        .iter()
        .find(|source| package["name"] == source.name && package["version"] == source.version)
    else {
        return Ok(0);
    };
    let vcs_path = directory.join(".cargo_vcs_info.json");
    regular(&vcs_path)?;
    let vcs: serde_json::Value = serde_json::from_slice(&fs::read(vcs_path)?)?;
    if package["repository"] != source.repository
        || vcs["git"]["sha1"] != source.revision
        || vcs["path_in_vcs"] != source.source_directory
        || vcs["git"]["dirty"] == true
    {
        return Err(format!(
            "supplemental notice source does not match {} {}",
            source.name, source.version
        )
        .into());
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../third_party/windows-distribution-dependencies")
        .join(&source.notice_directory);
    for (name, expected) in &source.files {
        let path = root.join(name);
        regular(&path)?;
        if format!("{:x}", Sha256::digest(fs::read(&path)?)) != *expected {
            return Err(format!(
                "supplemental notice hash differs for {} {name}",
                source.name
            )
            .into());
        }
        copy(&path, &destination.join(name))?;
    }
    Ok(source.files.len())
}

fn installer_notice(root: &Path, compiler: &Path) -> Result<serde_json::Value> {
    regular(compiler)?;
    let source = compiler
        .parent()
        .ok_or("NSIS compiler directory is missing")?
        .join("COPYING");
    regular(&source)?;
    let destination = root.join("nsis-3.13/COPYING");
    copy(&source, &destination)?;
    Ok(serde_json::json!({
        "ecosystem":"installer", "name":"NSIS", "version":"3.13",
        "source":"https://nsis.sourceforge.io/License",
        "license":"See exact installed NSIS COPYING",
        "notice_files":1,
        "notice_sha256":format!("{:x}", Sha256::digest(fs::read(&destination)?)),
        "compiler_sha256":format!("{:x}", Sha256::digest(fs::read(compiler)?))
    }))
}

fn notices(root: &Path, compiler: Option<&Path>) -> Result<()> {
    let metadata: serde_json::Value =
        serde_json::from_str(&output(Command::new(std::env::var("CARGO")?).args([
            "metadata",
            "--locked",
            "--format-version",
            "1",
            "--filter-platform",
            TARGET,
        ]))?)?;
    let packages = metadata["packages"]
        .as_array()
        .ok_or("Cargo packages are missing")?;
    let nodes = metadata["resolve"]["nodes"]
        .as_array()
        .ok_or("Cargo resolution is missing")?;
    let domus = packages
        .iter()
        .find(|package| package["name"] == "domus")
        .ok_or("domus package missing")?;
    let mut pending = vec![domus["id"].as_str().ok_or("domus id missing")?.to_owned()];
    let mut included = BTreeSet::new();
    while let Some(id) = pending.pop() {
        if !included.insert(id.clone()) {
            continue;
        }
        let node = nodes
            .iter()
            .find(|node| node["id"] == id)
            .ok_or("Cargo dependency node missing")?;
        for dependency in node["deps"]
            .as_array()
            .ok_or("Cargo dependencies missing")?
        {
            if dependency["dep_kinds"]
                .as_array()
                .ok_or("Cargo dependency kinds missing")?
                .iter()
                .any(|kind| kind["kind"] != "dev")
            {
                pending.push(
                    dependency["pkg"]
                        .as_str()
                        .ok_or("Cargo dependency id missing")?
                        .into(),
                );
            }
        }
    }
    let mut index = Vec::new();
    for package in packages {
        if !included.contains(package["id"].as_str().ok_or("Cargo package id missing")?) {
            continue;
        }
        let name = package["name"]
            .as_str()
            .ok_or("Cargo package name missing")?;
        let version = package["version"]
            .as_str()
            .ok_or("Cargo package version missing")?;
        let directory = Path::new(
            package["manifest_path"]
                .as_str()
                .ok_or("Cargo manifest path missing")?,
        )
        .parent()
        .ok_or("Cargo package directory missing")?;
        let destination = root.join("rust").join(format!("{name}-{version}"));
        let mut count = notice_files(directory, &destination)?;
        if package["source"].is_string() && count == 0 {
            count = supplemental_notices(package, directory, &destination)?;
        }
        if package["source"].is_string() && count == 0 {
            return Err(format!(
                "distribution notice missing for {name} {version}; resolve through #26"
            )
            .into());
        }
        index.push(serde_json::json!({
            "ecosystem":"rust", "name":name, "version":version,
            "source":package["source"], "license":package["license"],
            "notice_files":count
        }));
    }
    let frontend: serde_json::Value = serde_json::from_str(&output(
        Command::new("pnpm.cmd")
            .args(["--dir", "frontend", "licenses", "list", "--prod", "--json"]),
    )?)?;
    for group in frontend
        .as_object()
        .ok_or("frontend license groups missing")?
        .values()
    {
        for package in group.as_array().ok_or("frontend license entries missing")? {
            let name = package["name"].as_str().ok_or("frontend name missing")?;
            let versions = package["versions"]
                .as_array()
                .ok_or("frontend versions missing")?;
            let paths = package["paths"]
                .as_array()
                .ok_or("frontend package paths missing")?;
            if versions.len() != 1 || paths.len() != 1 {
                return Err(
                    "frontend notice collection requires one exact resolved version per component"
                        .into(),
                );
            }
            let version = versions[0].as_str().ok_or("frontend version missing")?;
            let source = Path::new(paths[0].as_str().ok_or("frontend package path missing")?);
            if notice_files(
                source,
                &root.join("frontend").join(format!("{name}-{version}")),
            )? == 0
            {
                return Err(format!("frontend distribution notice missing for {name}").into());
            }
            index.push(serde_json::json!({
                "ecosystem":"frontend", "name":name, "version":version, "license":package["license"]
            }));
        }
    }
    copy(
        Path::new("frontend/app/assets/OFL.txt"),
        &root.join("font/OFL.txt"),
    )?;
    copy(
        Path::new("third_party/file-id-0.2.3"),
        &root.join("file-id-provenance"),
    )?;
    copy(
        Path::new("third_party/certificate-dependencies"),
        &root.join("certificate-dependencies"),
    )?;
    copy(
        Path::new("third_party/windows-distribution-dependencies/sources.json"),
        &root.join("supplemental-sources.json"),
    )?;
    let jpeg = Path::new("target/native/src/libjpeg-turbo-3.2.0");
    if notice_files(jpeg, &root.join("libjpeg-turbo-3.2.0"))? == 0 {
        return Err("native JPEG notices are missing".into());
    }
    for component in ["src/spng", "src/spng/zlib"] {
        notice_files(
            &jpeg.join(component),
            &root.join("libjpeg-turbo-3.2.0").join(component),
        )?;
    }
    if let Some(compiler) = compiler {
        index.push(installer_notice(root, compiler)?);
    }
    fs::write(
        root.join("components.json"),
        serde_json::to_vec_pretty(&index)?,
    )?;
    Ok(())
}

fn imports(executable: &Path) -> Result<Vec<String>> {
    let text = output(
        Command::new("dumpbin.exe")
            .arg("/nologo")
            .arg("/dependents")
            .arg(executable),
    )?;
    let mut result = BTreeSet::new();
    for line in text.lines() {
        let name = line.trim().to_ascii_lowercase();
        if name.ends_with(".dll") {
            if !system_dll(&name) {
                return Err(format!("unpackaged runtime dependency: {name}").into());
            }
            result.insert(name);
        }
    }
    if result.is_empty() {
        return Err("PE dependency inspection returned no imports".into());
    }
    Ok(result.into_iter().collect())
}

fn system_dll(name: &str) -> bool {
    name.starts_with("api-ms-win-core-")
        || name.starts_with("ext-ms-win-")
        || matches!(
            name,
            "kernel32.dll"
                | "ntdll.dll"
                | "advapi32.dll"
                | "bcrypt.dll"
                | "bcryptprimitives.dll"
                | "crypt32.dll"
                | "ncrypt.dll"
                | "userenv.dll"
                | "ws2_32.dll"
                | "user32.dll"
                | "gdi32.dll"
                | "shell32.dll"
                | "ole32.dll"
                | "oleaut32.dll"
                | "secur32.dll"
                | "rpcrt4.dll"
                | "iphlpapi.dll"
                | "dnsapi.dll"
                | "normaliz.dll"
                | "netapi32.dll"
                | "synchronization.dll"
                | "wintrust.dll"
                | "winmm.dll"
        )
}

pub fn verify(root: &Path) -> Result<()> {
    verified_files(root).map(|_| ())
}

fn verified_files(root: &Path) -> Result<BTreeMap<String, String>> {
    let manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(root.join("package-manifest.json"))?)?;
    if manifest["target"] != TARGET || manifest["format"] != 1 {
        return Err("unsupported Windows package manifest".into());
    }

    let paths = files(root)?;
    let observed = serde_json::to_value(&paths)?;
    if observed != manifest["files"] {
        return Err(
            "Windows payload differs from its manifest; changed and unrelated files are preserved"
                .into(),
        );
    }
    for required in [
        "domus.exe",
        "browser/index.html",
        "services.ps1",
        "windows-services.md",
        "notices/components.json",
    ] {
        if observed.get(required).is_none() {
            return Err(format!("Windows payload is missing {required}").into());
        }
    }
    imports(&root.join("domus.exe"))?;
    output(Command::new(root.join("domus.exe")).arg("version"))?;
    Ok(paths)
}

pub fn smoke(root: &Path) -> Result<()> {
    let shell = PathBuf::from(std::env::var_os("SystemRoot").ok_or("SystemRoot is missing")?)
        .join("System32\\WindowsPowerShell\\v1.0\\powershell.exe");
    output(
        Command::new(shell)
            .args([
                "-NoProfile",
                "-File",
                "packaging/windows/package-smoke.ps1",
                "-Payload",
            ])
            .arg(std::path::absolute(root)?),
    )?;
    println!(
        "Copied Windows payload served the real browser and retained state after forced exit."
    );
    Ok(())
}

fn clear_previous(root: &Path) -> Result<()> {
    if !root.exists() {
        return Ok(());
    }
    let paths = verified_files(root)?;
    clear_verified(root, paths)
}

fn clear_verified(root: &Path, paths: BTreeMap<String, String>) -> Result<()> {
    let mut directories = BTreeSet::<PathBuf>::new();
    for (name, expected) in &paths {
        let path = root.join(name);
        for parent in path.ancestors().skip(1).take_while(|path| *path != root) {
            regular(parent)?;
            directories.insert(parent.to_owned());
        }
        regular(root)?;
        regular(&path)?;
        if fs::metadata(&path)?.len() > 256 * 1024 * 1024
            || format!("{:x}", Sha256::digest(fs::read(&path)?)) != *expected
        {
            return Err("verified package file changed before cleanup; it was preserved".into());
        }
        fs::remove_file(path)?;
    }
    fs::remove_file(root.join("package-manifest.json"))?;
    for path in directories.iter().rev() {
        fs::remove_dir(path)?;
    }
    fs::remove_dir(root)?;
    Ok(())
}

fn uninstall_include(mut owned: BTreeMap<String, String>) -> Result<String> {
    if owned.len() > MAX_FILES - 2 {
        return Err("payload must leave room for the generated uninstall files".into());
    }
    if owned.keys().any(|name| {
        [
            "Uninstall.exe",
            "package-manifest.json",
            "uninstall-files.nsh",
        ]
        .iter()
        .any(|reserved| name.eq_ignore_ascii_case(reserved))
    }) {
        return Err("payload conflicts with a generated installer file".into());
    }
    owned.insert("package-manifest.json".into(), String::new());
    owned.insert("uninstall-files.nsh".into(), String::new());
    let mut uninstall = String::new();
    let mut directories = BTreeSet::new();
    let mut names = BTreeSet::new();
    for name in owned.keys() {
        if !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"/._-".contains(&byte))
            || !names.insert(name.to_ascii_lowercase())
            || name.split('/').any(|part| {
                let stem = part.split_once('.').map_or(part, |(stem, _)| stem);
                let stem = stem.to_ascii_uppercase();
                part.is_empty()
                    || matches!(part, "." | "..")
                    || part.ends_with('.')
                    || matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
                    || (stem.len() == 4
                        && (stem.starts_with("COM") || stem.starts_with("LPT"))
                        && matches!(stem.as_bytes()[3], b'1'..=b'9'))
            })
        {
            return Err("payload path cannot be represented safely in NSIS".into());
        }
        uninstall.push_str(&format!(
            "!insertmacro DeleteChecked \"$INSTDIR\\{}\" removal_failed\n",
            name.replace('/', "\\")
        ));
        for parent in Path::new(name)
            .ancestors()
            .skip(1)
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            directories.insert(parent.to_string_lossy().replace('/', "\\"));
        }
    }
    for directory in directories.iter().rev() {
        uninstall.push_str(&format!("RMDir \"$INSTDIR\\{directory}\"\n"));
    }
    Ok(uninstall)
}

pub fn build(installer: bool) -> Result<()> {
    if !cfg!(windows) {
        return Err("Windows packaging must run on native Windows x64 MSVC".into());
    }
    let host = output(Command::new("rustc").arg("-vV"))?;
    if !host.lines().any(|line| line == format!("host: {TARGET}")) {
        return Err("Windows packaging requires native x64 MSVC Rust".into());
    }
    let compiler = if installer {
        let requested = std::env::var_os("MAKENSIS").unwrap_or_else(|| "makensis.exe".into());
        let requested = PathBuf::from(requested);
        let compiler = if requested.is_absolute() {
            requested
        } else {
            let found = output(Command::new("where.exe").arg(&requested))?;
            PathBuf::from(
                found
                    .lines()
                    .next()
                    .ok_or("NSIS compiler is not available; set MAKENSIS to its executable")?,
            )
        };
        regular(&compiler)?;
        let version = output(Command::new(&compiler).arg("/VERSION"))?;
        if version.trim() != "v3.13" {
            return Err("NSIS 3.13 is required".into());
        }
        Some(compiler)
    } else {
        None
    };
    let mut build = Command::new(std::env::var("CARGO")?);
    build
        .args([
            "build",
            "--locked",
            "--release",
            "--package",
            "domus",
            "--target",
            TARGET,
        ])
        .env("RUSTFLAGS", "-C target-feature=+crt-static");
    if std::env::var_os("RUSTFLAGS").is_some()
        || std::env::var_os("CARGO_ENCODED_RUSTFLAGS").is_some()
    {
        return Err("package builds require an unset RUSTFLAGS and CARGO_ENCODED_RUSTFLAGS for a known link configuration".into());
    }
    super::native_jpeg::configure_static(&mut build)?;
    let status = build.status()?;
    if !status.success() {
        return Err("Windows release compilation failed".into());
    }
    let final_root = Path::new("target/windows-package");
    let staging =
        Path::new("target").join(format!("windows-package-{}.pending", std::process::id()));
    let root = staging.as_path();
    fs::create_dir(root)?;
    println!("Staging Windows payload at {}.", root.display());
    copy(
        &Path::new("target").join(TARGET).join("release/domus.exe"),
        &root.join("domus.exe"),
    )?;
    copy(Path::new("frontend/dist-app"), &root.join("browser"))?;
    copy(
        Path::new("crates/domus/migrations"),
        &root.join("migrations"),
    )?;
    copy(
        Path::new("packaging/windows/services.ps1"),
        &root.join("services.ps1"),
    )?;
    copy(
        Path::new("docs/windows-services.md"),
        &root.join("windows-services.md"),
    )?;
    for file in [
        "Cargo.lock",
        "pnpm-lock.yaml",
        "rust-toolchain.toml",
        ".node-version",
        "package.json",
        "xtask/native-jpeg.cmake",
    ] {
        copy(Path::new(file), &root.join("build-provenance").join(file))?;
    }
    let runtime_imports = imports(&root.join("domus.exe"))?;
    smoke(root)?;
    notices(&root.join("notices"), compiler.as_deref())?;
    fs::write(
        root.join("uninstall-files.nsh"),
        uninstall_include(files(root)?)?,
    )?;
    let revision = output(Command::new("git").args(["rev-parse", "HEAD"]))?;
    let dirty = output(Command::new("git").args(["status", "--porcelain"]))?;
    let manifest = serde_json::json!({
        "format":1, "target":TARGET, "version":env!("CARGO_PKG_VERSION"),
        "revision":revision.trim(), "dirty":!dirty.is_empty(),
        "rust":output(Command::new("rustc").arg("--version"))?.trim(),
        "node":output(Command::new("node").arg("--version"))?.trim(),
        "crt":"static", "imports":runtime_imports,
        "notice_review":"Collected source notices; independent distribution review remains #26.",
        "files":files(root)?
    });
    fs::write(
        root.join("package-manifest.json"),
        serde_json::to_vec_pretty(&manifest)?,
    )?;
    verify(root)?;
    clear_previous(final_root)?;
    fs::rename(root, final_root)?;
    let root = final_root;
    if let Some(compiler) = compiler {
        output(
            Command::new(compiler)
                .arg("/NOCONFIG")
                .arg("/WX")
                .arg(format!(
                    "/DPAYLOAD={}",
                    std::path::absolute(root)?.display()
                ))
                .arg(format!(
                    "/DOUTPUT={}",
                    std::path::absolute("target")?
                        .join("DomusObscuraSetup.exe")
                        .display()
                ))
                .arg("packaging/windows/installer.nsi"),
        )?;
    }
    println!(
        "Verified Windows candidate at {}. Native SCM and distribution review are separate acceptance checks.",
        root.display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_package_installer_notices_require_the_exact_compiler_source() -> Result<()> {
        let source = tempfile::tempdir()?;
        let destination = tempfile::tempdir()?;
        let compiler = source.path().join("makensis.exe");
        fs::write(&compiler, b"fixture compiler")?;
        assert!(installer_notice(destination.path(), &compiler).is_err());
        fs::write(source.path().join("COPYING"), b"fixture notice")?;
        let component = installer_notice(destination.path(), &compiler)?;
        assert_eq!(component["name"], "NSIS");
        assert_eq!(component["version"], "3.13");
        assert_eq!(
            fs::read(destination.path().join("nsis-3.13/COPYING"))?,
            b"fixture notice"
        );
        assert_eq!(
            component["notice_sha256"],
            format!("{:x}", Sha256::digest(b"fixture notice"))
        );
        assert_eq!(
            component["compiler_sha256"],
            format!("{:x}", Sha256::digest(b"fixture compiler"))
        );
        Ok(())
    }

    #[test]
    fn windows_package_rejects_external_runtime_dependencies() {
        for name in [
            "kernel32.dll",
            "ws2_32.dll",
            "bcryptprimitives.dll",
            "api-ms-win-core-file-l1-1-0.dll",
        ] {
            assert!(system_dll(name));
        }
        for name in [
            "vcruntime140.dll",
            "msvcp140.dll",
            "turbojpeg.dll",
            "custom.dll",
            "api-ms-win-crt-runtime-l1-1-0.dll",
        ] {
            assert!(!system_dll(name));
        }
    }

    #[test]
    fn windows_package_hashes_exact_payload_and_preserves_unknown_files() -> Result<()> {
        let root = tempfile::tempdir()?;
        fs::write(root.path().join("file.txt"), "one")?;
        let before = files(root.path())?;
        fs::write(root.path().join("file.txt"), "two")?;
        assert_ne!(before, files(root.path())?);
        fs::write(root.path().join("unrelated.txt"), "keep")?;
        assert!(clear_previous(root.path()).is_err());
        assert_eq!(
            fs::read_to_string(root.path().join("unrelated.txt"))?,
            "keep"
        );
        Ok(())
    }

    #[test]
    fn windows_package_uninstall_lists_exact_files_without_recursive_removal() -> Result<()> {
        let include = uninstall_include(
            ["domus.exe", "browser/assets/app.js", "browser/index.html"]
                .into_iter()
                .map(|name| (name.into(), String::new()))
                .collect(),
        )?;
        assert_eq!(
            include,
            concat!(
                "!insertmacro DeleteChecked \"$INSTDIR\\browser\\assets\\app.js\" removal_failed\n",
                "!insertmacro DeleteChecked \"$INSTDIR\\browser\\index.html\" removal_failed\n",
                "!insertmacro DeleteChecked \"$INSTDIR\\domus.exe\" removal_failed\n",
                "!insertmacro DeleteChecked \"$INSTDIR\\package-manifest.json\" removal_failed\n",
                "!insertmacro DeleteChecked \"$INSTDIR\\uninstall-files.nsh\" removal_failed\n",
                "RMDir \"$INSTDIR\\browser\\assets\"\n",
                "RMDir \"$INSTDIR\\browser\"\n"
            )
        );
        for name in [
            "../escape.txt",
            "browser/./alias.txt",
            "browser//alias.txt",
            "browser/file.",
            "CON.txt",
            "COM1.bin",
            "$INSTDIR.txt",
            "line\nbreak.txt",
            "Uninstall.exe",
            "UNINSTALL-FILES.NSH",
        ] {
            assert!(uninstall_include([(name.into(), String::new())].into()).is_err());
        }
        assert!(
            uninstall_include(
                [
                    ("file.txt".into(), String::new()),
                    ("FILE.txt".into(), String::new()),
                ]
                .into()
            )
            .is_err()
        );
        Ok(())
    }

    #[test]
    fn windows_package_reserves_installed_uninstaller_capacity() {
        let files = (0..MAX_FILES - 2)
            .map(|index| (format!("file-{index}.txt"), String::new()))
            .collect::<BTreeMap<_, _>>();
        assert!(uninstall_include(files.clone()).is_ok());
        let mut oversized = files;
        oversized.insert("extra.txt".into(), String::new());
        assert!(uninstall_include(oversized).is_err());
    }

    #[test]
    fn windows_package_supplemental_notices_require_exact_source() -> Result<()> {
        let input = tempfile::tempdir()?;
        let output = tempfile::tempdir()?;
        let mut package = serde_json::json!({
            "name": "ts-rs",
            "version": "12.0.1",
            "repository": "https://github.com/Aleph-Alpha/ts-rs"
        });
        let mut vcs = serde_json::json!({
            "git": {"sha1": "7182ad8289596097235406b715fa04506443b4ad"},
            "path_in_vcs": "ts-rs"
        });
        let vcs_path = input.path().join(".cargo_vcs_info.json");
        fs::write(&vcs_path, serde_json::to_vec(&vcs)?)?;
        assert_eq!(
            supplemental_notices(&package, input.path(), output.path())?,
            1
        );
        let expected = fs::read(output.path().join("LICENSE"))?;
        vcs["git"]["sha1"] = "another-revision".into();
        fs::write(&vcs_path, serde_json::to_vec(&vcs)?)?;
        assert!(supplemental_notices(&package, input.path(), output.path()).is_err());
        vcs["git"]["sha1"] = "7182ad8289596097235406b715fa04506443b4ad".into();
        fs::write(&vcs_path, serde_json::to_vec(&vcs)?)?;
        package["repository"] = "https://github.com/another/source".into();
        assert!(supplemental_notices(&package, input.path(), output.path()).is_err());
        assert_eq!(fs::read(output.path().join("LICENSE"))?, expected);
        package["version"] = "12.0.2".into();
        assert_eq!(
            supplemental_notices(&package, input.path(), output.path())?,
            0
        );
        Ok(())
    }
    #[test]
    fn windows_package_cleanup_preserves_files_added_or_changed_after_verification() -> Result<()> {
        let root = tempfile::tempdir()?;
        let owned = root.path().join("owned");
        fs::write(&owned, b"owned")?;
        fs::write(root.path().join("package-manifest.json"), b"fixture")?;
        let verified = files(root.path())?;
        fs::write(root.path().join("unrelated"), b"keep")?;
        assert!(clear_verified(root.path(), verified).is_err());
        assert_eq!(fs::read(root.path().join("unrelated"))?, b"keep");

        fs::write(&owned, b"owned")?;
        fs::write(root.path().join("package-manifest.json"), b"fixture")?;
        let verified = files(root.path())?;
        fs::write(&owned, b"changed")?;
        assert!(clear_verified(root.path(), verified).is_err());
        assert_eq!(fs::read(&owned)?, b"changed");
        assert_eq!(fs::read(root.path().join("unrelated"))?, b"keep");
        Ok(())
    }
}
