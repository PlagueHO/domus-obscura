import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("..", import.meta.url));
const args = process.argv.slice(2);
if (args.length === 0) {
  throw new Error("Select a developer task.");
}
const child =
  process.platform === "win32"
    ? spawn("pwsh", ["-NoProfile", "-File", "dev.ps1", ...args], {
        cwd: root,
        stdio: "inherit",
        shell: false,
      })
    : spawn("cargo", ["task", ...args], {
        cwd: root,
        stdio: "inherit",
        shell: false,
      });
child.on("error", (error) => {
  console.error(`Developer launcher failed: ${error.message}`);
  process.exitCode = 1;
});
child.on("exit", (code, signal) => {
  if (signal) {
    console.error(`Developer task exited after signal ${signal}.`);
  }
  process.exitCode = code ?? 1;
});
