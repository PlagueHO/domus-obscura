"""Export one source-bound test executable from the existing build image."""

import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys


def export_executable(artifact, output):
    shutil.copyfile(artifact, output)
    subprocess.run(["strip", "--strip-debug", str(output)], check=True, timeout=120)
    if output.stat().st_size > 512 * 1024 * 1024:
        raise ValueError("The exported media fixture executable is too large")
    output.chmod(0o755)


def main():
    revision = os.environ["DOMUS_MEDIA_FIXTURE_REVISION"]
    if not re.fullmatch(r"[0-9a-f]{40}", revision):
        raise ValueError("The media fixture requires an exact source revision")
    result = subprocess.run(
        ["cargo", "task", "test", "--lib",
         "--no-run", "--message-format=json"],
        text=True, stdout=subprocess.PIPE, check=False, timeout=1800,
    )
    if len(result.stdout) > 4 * 1024 * 1024:
        raise ValueError("Cargo fixture metadata exceeds its limit")
    artifacts = []
    for line in result.stdout.splitlines():
        if not line.startswith("{"):
            continue
        event = json.loads(line)
        if result.returncode and event.get("reason") == "compiler-message":
            message = event["message"]
            print(message.get("rendered") or json.dumps(message), file=sys.stderr)
        if (event.get("reason") == "compiler-artifact"
                and event.get("target", {}).get("name") == "domus"
                and event.get("profile", {}).get("test")
                and event.get("executable")):
            artifacts.append(Path(event["executable"]).resolve())
    result.check_returncode()
    if len(artifacts) != 1:
        raise ValueError("Exactly one domus test executable is required")
    artifact = artifacts[0]
    artifact.relative_to(Path("target").resolve())
    if not artifact.is_file() or artifact.stat().st_size > 512 * 1024 * 1024:
        raise ValueError("The media fixture executable is missing or too large")
    output = Path("/verification/media-workflow")
    export_executable(artifact, output)
    output.with_suffix(".revision").write_text(revision + "\n", encoding="utf-8")


if __name__ == "__main__":
    main()
