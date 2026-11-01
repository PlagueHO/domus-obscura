use std::env;
use std::error::Error;
use std::path::Path;
use std::process::{Command, ExitCode};

mod native_jpeg;

fn execute(program: &str, args: &[&str]) -> Result<(), Box<dyn Error>> {
    println!("> {program} {}", args.join(" "));
    let status = Command::new(program).args(args).status()?;
    if !status.success() {
        return Err(format!("{program} failed with {status}").into());
    }
    Ok(())
}

fn cargo(args: &[&str]) -> Result<(), Box<dyn Error>> {
    let program = env::var("CARGO")?;
    let mut command = Command::new(&program);
    command.args(args);
    if args.first() != Some(&"fmt") {
        native_jpeg::configure(&mut command)?;
    }
    println!("> {program} {}", args.join(" "));
    let status = command.status()?;
    if !status.success() {
        return Err(format!("{program} failed with {status}").into());
    }
    Ok(())
}

fn pnpm(args: &[&str]) -> Result<(), Box<dyn Error>> {
    execute(if cfg!(windows) { "pnpm.cmd" } else { "pnpm" }, args)
}

fn contracts(check: bool) -> Result<(), Box<dyn Error>> {
    cargo(&[
        "run",
        "--locked",
        "--quiet",
        "--package",
        "domus",
        "--",
        if check {
            "contracts-check"
        } else {
            "contracts"
        },
        "frontend/src/generated",
    ])
}

fn require_version(program: &str, expected: &str) -> Result<(), Box<dyn Error>> {
    let output = Command::new(program).arg("--version").output()?;
    let actual = String::from_utf8(output.stdout)?;
    if !output.status.success()
        || !actual
            .split_whitespace()
            .any(|part| part.trim_start_matches('v') == expected)
    {
        return Err(format!("{program} must be {expected}; received {}", actual.trim()).into());
    }
    Ok(())
}

fn toolchains() -> Result<(), Box<dyn Error>> {
    require_version("rustc", env!("CARGO_PKG_RUST_VERSION"))?;
    let node = std::fs::read_to_string(".node-version")?;
    require_version("node", node.trim())?;
    let package: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string("package.json")?)?;
    let manager = package["packageManager"]
        .as_str()
        .ok_or("missing packageManager")?;
    let version = manager
        .strip_prefix("pnpm@")
        .ok_or("packageManager must pin pnpm")?;
    require_version(if cfg!(windows) { "pnpm.cmd" } else { "pnpm" }, version)
}

fn run(args: &[String]) -> Result<(), Box<dyn Error>> {
    env::set_current_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join(".."))?;
    match args.first().map(String::as_str) {
        Some("bootstrap") => {
            toolchains()?;
            pnpm(&["install", "--frozen-lockfile"])
        }
        Some("contracts") => contracts(false),
        Some("local-test") => {
            toolchains()?;
            pnpm(&["--dir", "frontend", "run", "build:local-test"])?;
            cargo(&["run", "--locked", "--package", "domus", "--example", "local_test"])
        }
        Some("local-test-build") => {
            toolchains()?;
            pnpm(&["--dir", "frontend", "run", "build:local-test"])?;
            cargo(&["build", "--locked", "--package", "domus", "--example", "local_test"])
        }
        Some("fmt") => {
            cargo(&["fmt", "--all"])?;
            pnpm(&["format"])
        }
        Some("check") => {
            toolchains()?;
            contracts(true)?;
            cargo(&["fmt", "--all", "--check"])?;
            cargo(&[
                "clippy",
                "--locked",
                "--workspace",
                "--all-targets",
                "--",
                "-D",
                "warnings",
            ])?;
            cargo(&["test", "--locked", "--workspace"])?;
            cargo(&["test", "--locked", "--package", "domus", "--example", "local_test"])?;
            pnpm(&["format:check"])?;
            pnpm(&["--dir", "frontend", "run", "check"])?;
            Ok(())
        }
        Some("test") => {
            let mut test_args = vec!["test", "--locked", "--workspace"];
            test_args.extend(args.iter().skip(1).map(String::as_str));
            cargo(&test_args)
        }
        Some("build") => {
            toolchains()?;
            contracts(true)?;
            pnpm(&["--dir", "frontend", "run", "build"])?;
            pnpm(&["--dir", "frontend", "run", "build:app"])?;
            cargo(&["build", "--locked", "--package", "domus"])
        }
        Some("smoke") => cargo(&[
            "run",
            "--locked",
            "--quiet",
            "--package",
            "domus",
            "--",
            "version",
        ]),
        Some("scale") => cargo(&[
            "test",
            "--locked",
            "--package",
            "domus",
            "--test",
            "scale",
            "--",
            "--ignored",
        ]),
        _ => Err(
            "Usage: cargo task <bootstrap|contracts|fmt|check|test [filter]|scale|build|smoke|local-test|local-test-build>"
                .into(),
        ),
    }
}

fn main() -> ExitCode {
    match run(&env::args().skip(1).collect::<Vec<_>>()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}
