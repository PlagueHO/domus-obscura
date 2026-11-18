import { spawn, spawnSync } from "node:child_process";
import { createHash, randomUUID } from "node:crypto";
import { lstatSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";

const [binaryArgument, image, outputArgument] = process.argv.slice(2);
if (!binaryArgument || !image || !outputArgument || process.argv.length !== 5) {
  throw new Error(
    "Usage: node storage-container-proof.mjs <Linux-test-executable> <cached-runtime-image> <new-evidence-directory>",
  );
}
const binary = resolve(binaryArgument);
const output = resolve(outputArgument);
if (!lstatSync(binary).isFile() || lstatSync(binary).isSymbolicLink() || /[,\r\n]/u.test(binary)) {
  throw new Error("The executable must be a regular file with a safe mount path.");
}
if (image.startsWith("-") || /\s/u.test(image)) {
  throw new Error("Invalid runtime image.");
}
mkdirSync(output);
const id = randomUUID();
const label = "domus.storage-proof";
const volume = `domus-storage-proof-${id}`;
const containers = [];
let volumeCreated = false;
let failure;
const report = {
  executable_sha256: createHash("sha256").update(readFileSync(binary)).digest("hex"),
  image,
  id,
  volume,
  results: {},
  cleanup_errors: [],
};

function docker(args, timeout = 30000) {
  const result = spawnSync("docker", args, {
    encoding: "utf8",
    windowsHide: true,
    timeout,
    maxBuffer: 8 * 1024 * 1024,
  });
  if (result.error || result.status !== 0) {
    throw new Error(
      `docker ${args[0]} failed: ${result.error?.message ?? result.stderr ?? result.stdout}`,
    );
  }
  return (args[0] === "logs" ? result.stdout + result.stderr : result.stdout).trim();
}

function owned(kind, name) {
  const record = JSON.parse(docker([kind, "inspect", name]))[0];
  const labels = kind === "volume" ? record.Labels : record.Config.Labels;
  if (labels?.[label] !== id) {
    throw new Error(`Refusing a resource with a different owner: ${name}`);
  }
  return record;
}

function create(mode, target) {
  const name = `domus-storage-${mode}-${id}`;
  containers.push({ name, mode, target, removed: false });
  docker([
    "create",
    "--pull",
    "never",
    "--name",
    name,
    "--label",
    `${label}=${id}`,
    "--network",
    "none",
    "--read-only",
    "--cpus",
    "1",
    "--memory",
    "768m",
    "--pids-limit",
    "64",
    "--cap-drop",
    "ALL",
    "--security-opt",
    "no-new-privileges",
    "--mount",
    `type=volume,source=${volume},target=${target},volume-nocopy`,
    "--mount",
    `type=bind,source=${binary},target=/probe-executable,readonly`,
    "--env",
    `DOMUS_STORAGE_PROOF_ROOT=${target}`,
    "--env",
    `DOMUS_STORAGE_PROOF_MODE=${mode}`,
    "--entrypoint",
    "/probe-executable",
    image,
    "--exact",
    "infrastructure::store::native_storage_tests::logical_store_container_probe",
    "--ignored",
    "--nocapture",
    "--test-threads=1",
  ]);
  owned("container", name);
  return containers.at(-1);
}

function wait(container) {
  return new Promise((accept, reject) => {
    const child = spawn("docker", ["wait", container.name], {
      windowsHide: true,
      stdio: ["ignore", "pipe", "pipe"],
    });
    let stdout = "";
    let stderr = "";
    const timer = setTimeout(() => {
      child.kill();
      reject(new Error(`${container.mode} exceeded the bounded wait.`));
    }, 120000);
    child.stdout.on("data", (chunk) => {
      stdout += chunk;
    });
    child.stderr.on("data", (chunk) => {
      stderr += chunk;
    });
    child.on("error", (error) => {
      clearTimeout(timer);
      reject(error);
    });
    child.on("close", (code) => {
      clearTimeout(timer);
      if (code !== 0 || stdout.trim() !== "0") {
        reject(new Error(`${container.mode} failed: ${stdout} ${stderr}`));
      } else {
        accept();
      }
    });
  });
}

function result(container) {
  const state = owned("container", container.name).State;
  writeFileSync(resolve(output, `${container.mode}.log`), `${docker(["logs", container.name])}\n`, {
    flag: "wx",
  });
  if (state.Running || state.ExitCode !== 0) {
    throw new Error(`${container.mode} did not exit successfully.`);
  }
  const path = resolve(output, `${container.mode}-result.json`);
  docker(["cp", `${container.name}:${container.target}/${container.mode}-result.json`, path]);
  const value = JSON.parse(readFileSync(path, "utf8"));
  if (value.mode !== container.mode || value.rows !== (container.mode === "initialize" ? 1 : 21)) {
    throw new Error(`${container.mode} returned invalid evidence.`);
  }
  report.results[container.mode] = value;
}

function remove(container) {
  owned("container", container.name);
  docker(["rm", "--force", container.name]);
  container.removed = true;
}

try {
  if (docker(["info", "--format", "{{.OSType}}"]) !== "linux") {
    throw new Error("A Linux Docker engine is required.");
  }
  report.image_id = JSON.parse(docker(["image", "inspect", image]))[0].Id;
  volumeCreated = true;
  docker(["volume", "create", "--label", `${label}=${id}`, volume]);
  owned("volume", volume);
  const initial = create("initialize", "/initial/proof");
  docker(["start", initial.name]);
  await wait(initial);
  result(initial);
  const selectionBefore = resolve(output, "selection-before.json");
  docker(["cp", `${initial.name}:${initial.target}/app/selection.json`, selectionBefore]);
  remove(initial);
  const writer = create("writer", "/writer/proof");
  const reader = create("reader", "/reader/different/proof");
  docker(["start", writer.name]);
  docker(["start", reader.name]);
  const completed = await Promise.allSettled([wait(writer), wait(reader)]);
  result(writer);
  result(reader);
  for (const outcome of completed) {
    if (outcome.status === "rejected") throw outcome.reason;
  }
  remove(writer);
  remove(reader);
  const recreated = create("recreated", "/recreated/new/ancestor/proof");
  docker(["start", recreated.name]);
  await wait(recreated);
  result(recreated);
  const selectionAfter = resolve(output, "selection-after.json");
  docker(["cp", `${recreated.name}:${recreated.target}/app/selection.json`, selectionAfter]);
  if (!readFileSync(selectionBefore).equals(readFileSync(selectionAfter))) {
    throw new Error("The saved selection changed.");
  }
  const original = report.results.initialize;
  for (const [mode, value] of Object.entries(report.results)) {
    if (
      JSON.stringify(value.logical_root) !== JSON.stringify(original.logical_root) ||
      value.native_root.identity !== original.native_root.identity
    ) {
      throw new Error(`${mode} changed the saved binding or retained media identity.`);
    }
    if (
      mode !== "initialize" &&
      JSON.stringify(value.native_root.ancestors) === JSON.stringify(original.native_root.ancestors)
    ) {
      throw new Error(`${mode} did not reproduce changed runtime ancestry.`);
    }
  }
  report.selection_sha256 = createHash("sha256")
    .update(readFileSync(selectionBefore))
    .digest("hex");
  report.concurrent_snapshot_preserved = true;
  report.unchanged_binding_after_recreation = true;
  report.distinct_runtime_ancestors = true;
  remove(recreated);
} catch (error) {
  failure = error;
  report.error = error.message;
} finally {
  for (const container of containers.filter((entry) => !entry.removed)) {
    try {
      const state = owned("container", container.name).State;
      writeFileSync(
        resolve(output, `${container.mode}-failure-state.json`),
        `${JSON.stringify(state, null, 2)}\n`,
      );
      writeFileSync(
        resolve(output, `${container.mode}-failure.log`),
        `${docker(["logs", container.name])}\n`,
      );
      remove(container);
    } catch (error) {
      report.cleanup_errors.push(error.message);
    }
  }
  if (volumeCreated) {
    try {
      owned("volume", volume);
      docker(["volume", "rm", volume]);
      report.volume_removed = true;
    } catch (error) {
      report.cleanup_errors.push(error.message);
    }
  }
  report.success = !failure && report.cleanup_errors.length === 0;
  writeFileSync(resolve(output, "report.json"), `${JSON.stringify(report, null, 2)}\n`, {
    flag: "wx",
  });
}
console.log(JSON.stringify(report, null, 2));
if (!report.success) process.exitCode = 1;
