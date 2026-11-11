use std::env;
use std::error::Error;
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};

pub(super) fn run(full: bool) -> Result<(), Box<dyn Error>> {
    let python = if cfg!(windows) { "python" } else { "python3" };
    super::execute(python, &["scripts/physical-scale-tests.py"])?;
    let mut command = Command::new(env::var("CARGO")?);
    command.args([
        "test",
        "--locked",
        "--release",
        "--package",
        "domus",
        "--lib",
        "--no-run",
        "--message-format=json",
    ]);
    super::native_jpeg::configure(&mut command)?;
    let mut child = command.stdout(Stdio::piped()).spawn()?;
    let mut executable = None;
    let output = child.stdout.take().ok_or("missing compiler output")?;
    let parsed = (|| -> Result<(), Box<dyn Error>> {
        for line in BufReader::new(output).lines() {
            let line = line?;
            let message: serde_json::Value = serde_json::from_str(&line)?;
            if message["reason"] == "compiler-artifact"
                && message["target"]["name"] == "domus"
                && message["profile"]["test"] == true
            {
                if let Some(path) = message["executable"].as_str() {
                    executable = Some(path.to_owned());
                }
            } else if message["reason"] == "compiler-message"
                && let Some(rendered) = message["message"]["rendered"].as_str()
            {
                eprint!("{rendered}");
            }
        }
        Ok(())
    })();
    let status = child.wait()?;
    parsed?;
    if !status.success() {
        return Err("optimized physical-scale test host build failed".into());
    }
    let executable = executable.ok_or("compiler did not return the scale test host")?;
    let mut runner = Command::new(python);
    runner.args([
        "scripts/physical-scale.py",
        "run",
        "--test-host",
        &executable,
        "--profile",
        if full { "full" } else { "small" },
    ]);
    for (environment, argument) in [
        ("DOMUS_SCALE_OUTPUT", "--output"),
        ("DOMUS_SCALE_MANIFEST", "--manifest"),
        ("DOMUS_SCALE_ADMISSION", "--admission"),
    ] {
        if let Some(value) = env::var_os(environment) {
            runner.arg(argument).arg(value);
        }
    }
    if !runner.status()?.success() {
        return Err("physical-scale evidence failed; inspect the retained report".into());
    }
    Ok(())
}
