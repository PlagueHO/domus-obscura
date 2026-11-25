use std::env;
use std::error::Error;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::process::Stdio;

type TaskResult = Result<(), Box<dyn Error>>;

fn binary() -> Result<PathBuf, Box<dyn Error>> {
    let path = env::current_dir()?
        .join("target")
        .join("debug")
        .join(if cfg!(windows) { "domus.exe" } else { "domus" });
    if !path.is_file() {
        return Err("Build the real application with the build task first.".into());
    }
    Ok(path)
}

fn root() -> Result<PathBuf, Box<dyn Error>> {
    let root = env::var_os("DOMUS_DEV_ROOT")
        .map(PathBuf::from)
        .ok_or("Set DOMUS_DEV_ROOT to an explicit absolute developer parent directory.")?;
    if !root.is_absolute() {
        return Err("DOMUS_DEV_ROOT must be absolute.".into());
    }
    Ok(root)
}

fn configuration(args: &[String]) -> Result<PathBuf, Box<dyn Error>> {
    if args.len() > 1 {
        return Err("Pass at most one absolute configuration path.".into());
    }
    let path = match args.first() {
        Some(path) => PathBuf::from(path),
        None => match env::var_os("DOMUS_DEV_CONFIG") {
            Some(path) => PathBuf::from(path),
            None => root()?.join("application").join("config.json"),
        },
    };
    if !path.is_absolute() {
        return Err("The developer configuration path must be absolute.".into());
    }
    Ok(path)
}

fn execute(command: &mut Command) -> TaskResult {
    let status = command.status()?;
    if !status.success() {
        return Err(format!("Developer command failed: {status}").into());
    }
    Ok(())
}

pub fn package(operation: &str, args: &[String]) -> TaskResult {
    let mut command = Command::new("uv");
    command.args([
        "run",
        "--no-project",
        "--no-python-downloads",
        "--python",
        "3.14",
        "packaging/docker/check.py",
        operation,
    ]);
    if !args
        .iter()
        .any(|arg| arg == "--image" || arg.starts_with("--image="))
        && let Some(image) = env::var_os("DOMUS_IMAGE")
    {
        command.arg("--image").arg(image);
    }
    execute(command.args(args))
}

pub fn run(args: &[String]) -> TaskResult {
    match args[0].as_str() {
        "dev-doctor" => {
            if args.len() != 1 {
                return Err("dev-doctor takes no arguments.".into());
            }
            super::toolchains()?;
            execute(Command::new("cmake").arg("--version"))?;
            println!("Pinned build tools are available. No software was installed.");
            Ok(())
        }
        "dev-build" => super::run(&["build".into()]),
        "dev-setup" => {
            if args.get(1).map(String::as_str) != Some("--confirm-fresh") || args.len() != 2 {
                return Err(
                    "Use dev-setup --confirm-fresh after reviewing the selected new targets."
                        .into(),
                );
            }
            let root = root()?;
            if cfg!(windows) {
                execute(
                    Command::new("pwsh")
                        .args(["-NoProfile", "-File"])
                        .arg(Path::new("scripts").join("local-setup.ps1"))
                        .arg("-Directory")
                        .arg(root)
                        .args(["-UseExistingParent", "-SingleUserHttp", "-Port", "8080"]),
                )
            } else {
                execute(
                    Command::new(binary()?)
                        .args(["developer-init", "--confirm-fresh"])
                        .arg(root.join("application")),
                )
            }
        }
        "dev-inspect" | "dev-run" | "dev-stop" | "dev-restart" | "dev-status" | "dev-logs" => {
            let configuration = configuration(&args[1..])?;
            let binary = binary()?;
            match args[0].as_str() {
                "dev-inspect" => execute(
                    Command::new(binary)
                        .arg("developer-inspect")
                        .arg(configuration),
                ),
                "dev-run" => execute(Command::new(binary).arg("developer-run").arg(configuration)),
                "dev-restart" => {
                    execute(
                        Command::new(&binary)
                            .arg("developer-control")
                            .arg(&configuration)
                            .arg("stop"),
                    )?;
                    execute(Command::new(binary).arg("developer-run").arg(configuration))
                }
                operation => execute(
                    Command::new(binary)
                        .arg("developer-control")
                        .arg(configuration)
                        .arg(
                            operation
                                .strip_prefix("dev-")
                                .ok_or("Invalid developer operation.")?,
                        ),
                ),
            }
        }
        operation if operation.starts_with("dev-docker-") => docker(operation, &args[1..]),
        _ => Err("Unknown developer task.".into()),
    }
}

fn compose() -> Result<Command, Box<dyn Error>> {
    let project = env::var("DOMUS_DEV_DOCKER_PROJECT").map_err(
        |_| "Set DOMUS_DEV_DOCKER_PROJECT to the explicit persistent developer deployment name.",
    )?;
    if project.is_empty()
        || project.len() > 48
        || !project
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        || project.starts_with('-')
    {
        return Err(
            "The Docker project name must use lowercase letters, digits and internal hyphens."
                .into(),
        );
    }
    let mut command = Command::new("docker");
    command.args(["compose", "--project-name", &project, "-f", "compose.yaml"]);
    if let Some(override_file) = env::var_os("DOMUS_DEV_COMPOSE_OVERRIDE") {
        let path = PathBuf::from(override_file);
        if !path.is_absolute() || !path.is_file() {
            return Err("DOMUS_DEV_COMPOSE_OVERRIDE must select an existing absolute reviewed override file.".into());
        }
        command.arg("-f").arg(path);
    }
    command.args(["--profile", "worker"]);
    Ok(command)
}

fn docker(operation: &str, args: &[String]) -> TaskResult {
    if operation == "dev-docker-doctor" {
        if !args.is_empty() {
            return Err("Docker doctor takes no arguments.".into());
        }
        let output = Command::new("docker")
            .args(["version", "--format", "{{json .Server}}"])
            .output()?;
        if !output.status.success() {
            return Err("Docker Engine is unavailable; no settings were changed.".into());
        }
        let server: serde_json::Value = serde_json::from_slice(&output.stdout)?;
        if server["Os"] != "linux" || server["Arch"] != "amd64" {
            return Err("Developer Docker requires a linux/amd64 engine.".into());
        }
        execute(Command::new("docker").args(["compose", "version"]))?;
        println!(
            "Linux engine and Compose are available. Host networking, ext-family volume storage and host browser access still require verification."
        );
        return Ok(());
    }
    let mut command = compose()?;
    match operation {
        "dev-docker-setup" if args == ["--confirm-fresh"] => {
            require_stopped("worker")?;
            require_stopped("api")?;
            execute(command.args([
                "run",
                "--rm",
                "--no-deps",
                "--pull",
                "never",
                "api",
                "developer-init",
                "--confirm-fresh",
                "/storage/application",
            ]))
        }
        "dev-docker-setup" => {
            Err("Use dev-docker-setup --confirm-fresh for a new selected volume.".into())
        }
        _ if !args.is_empty() => {
            Err("This Docker task takes no arguments; use the local deployment selectors.".into())
        }
        "dev-docker-stop" => {
            let worker = graceful_stop("worker")?;
            let api = graceful_stop("api")?;
            if !worker || !api {
                return Err("A selected container exited unsuccessfully; inspect durable receipts before restarting.".into());
            }
            Ok(())
        }
        "dev-docker-restart" => {
            docker("dev-docker-stop", &[])?;
            docker("dev-docker-run", &[])
        }
        "dev-docker-run" => {
            require_stopped("worker")?;
            require_stopped("api")?;
            execute(command.args([
                "run",
                "--rm",
                "--no-deps",
                "--pull",
                "never",
                "api",
                "configuration-check",
                "/storage/application/config.json",
            ]))?;
            let probe = docker_probe()?;
            let origin = probe["origin"].as_str().ok_or("Probe origin missing.")?;
            let mut start = compose()?;
            start.env("DOMUS_HEALTH_URL", origin).args([
                "up",
                "--detach",
                "--pull",
                "never",
                "--no-build",
                "--wait",
                "--wait-timeout",
                "60",
                "api",
            ]);
            if probe["worker"] == true {
                start.arg("worker");
            }
            if let Err(error) = execute(&mut start).and_then(|()| verify_docker_probe(&probe)) {
                eprintln!("Docker startup failed: {error}");
                docker("dev-docker-stop", &[])?;
                return Err(error);
            }
            println!("Domus ready: {origin}");
            Ok(())
        }
        "dev-docker-status" => {
            execute(command.args(["ps", "--all"]))?;
            verify_docker_probe(&docker_probe()?)
        }
        "dev-docker-logs" => execute(command.args(["logs", "--tail", "100", "api", "worker"])),
        _ => Err("Unknown Docker developer task.".into()),
    }
}

fn require_stopped(service: &str) -> TaskResult {
    let output = compose()?
        .args(["ps", "--all", "--format", "json", service])
        .output()?;
    if !output.status.success() {
        return Err("Could not inspect Docker process state.".into());
    }
    check_stopped_containers(&output.stdout)
}

fn check_stopped_containers(bytes: &[u8]) -> TaskResult {
    for value in serde_json::Deserializer::from_slice(bytes).into_iter::<serde_json::Value>() {
        let value = value?;
        let states = match &value {
            serde_json::Value::Array(states) => states.as_slice(),
            _ => std::slice::from_ref(&value),
        };
        for state in states {
            if state["State"] != "exited" && state["State"] != "created" {
                return Err(
                    "A selected container is not stopped. Stop it before this command.".into(),
                );
            }
            if state["ExitCode"] != 0 {
                return Err("A selected container has an unsuccessful or unknown exit; inspect durable receipts before restarting.".into());
            }
        }
    }
    Ok(())
}

fn graceful_stop(service: &str) -> Result<bool, Box<dyn Error>> {
    let mut successful = true;
    let output = compose()?
        .args(["ps", "--all", "--quiet", service])
        .output()?;
    if !output.status.success() {
        return Err("Could not inspect Docker ownership.".into());
    }
    for id in String::from_utf8(output.stdout)?.split_whitespace() {
        let inspect = Command::new("docker")
            .args(["inspect", "--format", "{{json .State}}", id])
            .output()?;
        if !inspect.status.success() {
            return Err("Could not inspect the selected container.".into());
        }
        let state: serde_json::Value = serde_json::from_slice(&inspect.stdout)?;
        if state["Paused"] == true || state["Restarting"] == true {
            return Err(
                "Paused/restarting containers need explicit operator inspection before shutdown."
                    .into(),
            );
        }
        if state["Running"] == false {
            successful &= state["ExitCode"] == 0;
            continue;
        }
        if state["Running"] != true {
            return Err("Could not confirm the selected container running state.".into());
        }
        execute(Command::new("docker").args(["kill", "--signal", "SIGTERM", id]))?;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
        loop {
            let inspect = Command::new("docker")
                .args(["inspect", "--format", "{{json .State}}", id])
                .output()?;
            if !inspect.status.success() {
                return Err("Container exit could not be confirmed.".into());
            }
            let state: serde_json::Value = serde_json::from_slice(&inspect.stdout)?;
            if state["Running"] == false {
                successful &= state["ExitCode"] == 0;
                break;
            }
            if std::time::Instant::now() >= deadline {
                return Err(format!(
                    "{service} is still draining. No force kill was sent; do not restart."
                )
                .into());
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    }
    Ok(successful)
}

fn docker_probe() -> Result<serde_json::Value, Box<dyn Error>> {
    let output = compose()?
        .args([
            "run",
            "--rm",
            "--no-deps",
            "--pull",
            "never",
            "api",
            "developer-probe-info",
            "/storage/application/config.json",
        ])
        .output()?;
    if !output.status.success() {
        return Err(format!(
            "Docker probe configuration failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    Ok(serde_json::from_slice(&output.stdout)?)
}

fn verify_docker_probe(probe: &serde_json::Value) -> TaskResult {
    let mut child = Command::new(binary()?)
        .arg("developer-probe-remote")
        .stdin(Stdio::piped())
        .spawn()?;
    child
        .stdin
        .take()
        .ok_or("Probe stdin missing.")?
        .write_all(&serde_json::to_vec(probe)?)?;
    let status = child.wait()?;
    if !status.success() {
        return Err("Host browser/API/worker readiness failed; inspect Docker logs and supported host networking.".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn developer_docker_requires_confirmed_successful_stopped_containers() {
        for bytes in [
            b"".as_slice(),
            b"[]",
            br#"{"State":"exited","ExitCode":0}"#,
            br#"[{"State":"created","ExitCode":0},{"State":"exited","ExitCode":0}]"#,
            b"{\"State\":\"exited\",\"ExitCode\":0}\n{\"State\":\"created\",\"ExitCode\":0}\n",
        ] {
            assert!(check_stopped_containers(bytes).is_ok());
        }
        for bytes in [
            br#"{"State":"running","ExitCode":0}"#.as_slice(),
            br#"{"State":"exited","ExitCode":1}"#,
            br#"{"State":"exited"}"#,
            br#"[{"State":"exited","ExitCode":0},{"State":"paused","ExitCode":0}]"#,
            b"invalid",
        ] {
            assert!(check_stopped_containers(bytes).is_err());
        }
    }
}
