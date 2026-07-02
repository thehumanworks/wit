#!/usr/bin/env node

import fs from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);
const repoRoot = path.resolve(__dirname, "..", "..");
const CHECKSUM_FILE = "wit-checksums.txt";

function fail(message) {
  console.error(`error: ${message}`);
  process.exit(1);
}

function parseArgs(argv) {
  const args = {};

  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    if (arg === "--version") {
      args.version = argv[i + 1];
      i += 1;
      continue;
    }
    if (arg === "--artifacts-dir") {
      args.artifactsDir = argv[i + 1];
      i += 1;
      continue;
    }
    if (arg === "--output-dir") {
      args.outputDir = argv[i + 1];
      i += 1;
      continue;
    }
    fail(`unknown argument: ${arg}`);
  }

  if (!args.version) {
    fail("missing required --version");
  }
  if (!args.artifactsDir) {
    fail("missing required --artifacts-dir");
  }
  if (!args.outputDir) {
    fail("missing required --output-dir");
  }

  return args;
}

async function ensureDir(dirPath) {
  await fs.mkdir(dirPath, { recursive: true });
}

async function readTargetsConfig() {
  const configPath = path.join(repoRoot, "npm", "targets.json");
  const raw = await fs.readFile(configPath, "utf8");
  return JSON.parse(raw);
}

function checksumEntries(rawManifest) {
  return new Map(
    rawManifest
      .split(/\r?\n/)
      .map((line) => line.trim())
      .filter(Boolean)
      .map((line) => {
        const match = line.match(/^([a-f0-9]{64})\s+\*?(.+)$/i);
        if (!match) {
          fail(`invalid checksum entry: ${line}`);
        }
        return [match[2], match[1]];
      }),
  );
}

async function assertArtifactsExist(artifactsDir, targets) {
  const checksumsPath = path.join(artifactsDir, CHECKSUM_FILE);
  let rawChecksums;

  try {
    rawChecksums = await fs.readFile(checksumsPath, "utf8");
  } catch {
    fail(`checksum manifest missing: ${checksumsPath}`);
  }

  const checksums = checksumEntries(rawChecksums);

  for (const target of targets) {
    const artifactPath = path.join(artifactsDir, target.artifact);
    try {
      await fs.access(artifactPath);
    } catch {
      fail(`artifact missing for ${target.id}: ${artifactPath}`);
    }

    if (!checksums.has(target.artifact)) {
      fail(`checksum entry missing for ${target.artifact} in ${checksumsPath}`);
    }

    const archivedFiles = archiveFileBasenames(artifactPath);
    for (const binaryFile of [target.binaryFile, target.mcpBinaryFile].filter(Boolean)) {
      if (!archivedFiles.has(path.basename(binaryFile))) {
        fail(`artifact ${target.artifact} is missing ${binaryFile}`);
      }
    }
  }
}

function runCapture(command, args) {
  const result = spawnSync(command, args, { encoding: "utf8" });
  if (result.error) {
    fail(`failed to run ${command}: ${result.error.message}`);
  }
  if (result.status !== 0) {
    fail(
      `command failed (${result.status}): ${command} ${args.join(" ")}\n${result.stderr || ""}`,
    );
  }
  return result.stdout;
}

function archiveFileBasenames(artifactPath) {
  let listing;
  if (artifactPath.endsWith(".tar.gz")) {
    listing = runCapture("tar", ["-tzf", artifactPath]);
  } else if (artifactPath.endsWith(".zip")) {
    listing = runCapture("unzip", ["-Z1", artifactPath]);
  } else {
    fail(`unsupported artifact format: ${artifactPath}`);
  }

  return new Set(
    listing
      .split(/\r?\n/)
      .map((entry) => path.basename(entry.trim()))
      .filter(Boolean),
  );
}

function buildLauncher({ binaryName, packageName }, targets, binaryFileKey = "binaryFile") {
  const mapping = Object.fromEntries(
    targets.map((target) => [
      `${target.os}-${target.cpu}`,
      {
        binaryFile: target[binaryFileKey],
      },
    ]),
  );

  return `#!/usr/bin/env node
"use strict";

const path = require("node:path");
const { spawnSync } = require("node:child_process");

const TARGETS = ${JSON.stringify(mapping, null, 2)};
const PACKAGE_NAME = ${JSON.stringify(packageName)};

function fail(message) {
  console.error(message);
  process.exit(1);
}

function resolveBinaryPath() {
  const key = \`\${process.platform}-\${process.arch}\`;
  const target = TARGETS[key];
  if (!target) {
    fail(\`Unsupported platform/arch: \${process.platform}/\${process.arch}\`);
  }

  const binaryPath = path.join(__dirname, target.binaryFile);
  try {
    require("node:fs").accessSync(binaryPath);
    return binaryPath;
  } catch {
    fail(
      \`Binary not installed for \${process.platform}/\${process.arch}. Reinstall \${PACKAGE_NAME} without --ignore-scripts.\`,
    );
  }
}

const binaryPath = resolveBinaryPath();
const result = spawnSync(binaryPath, process.argv.slice(2), { stdio: "inherit" });

if (result.error) {
  console.error(result.error.message);
  process.exit(1);
}

if (result.signal) {
  process.kill(process.pid, result.signal);
}

process.exit(result.status ?? 1);
`;
}

function buildInstaller({ packageName, repositoryUrl }, targets) {
  const mapping = Object.fromEntries(
    targets.map((target) => [
      `${target.os}-${target.cpu}`,
      {
        artifact: target.artifact,
        binaryFile: target.binaryFile,
        binaryFiles: [target.binaryFile, target.mcpBinaryFile].filter(Boolean),
      },
    ]),
  );

  return `#!/usr/bin/env node
"use strict";

const fs = require("node:fs");
const fsp = require("node:fs/promises");
const os = require("node:os");
const path = require("node:path");
const { createHash } = require("node:crypto");
const { spawnSync } = require("node:child_process");
const packageJson = require("../package.json");

const TARGETS = ${JSON.stringify(mapping, null, 2)};
const PACKAGE_NAME = ${JSON.stringify(packageName)};
const REPOSITORY_URL = ${JSON.stringify(repositoryUrl)};
const CHECKSUM_FILE = ${JSON.stringify(CHECKSUM_FILE)};

function log(message) {
  console.log(\`\${PACKAGE_NAME}: \${message}\`);
}

function warn(message) {
  console.warn(\`\${PACKAGE_NAME}: \${message}\`);
}

function fail(message) {
  console.error(\`\${PACKAGE_NAME}: \${message}\`);
  process.exit(1);
}

function resolveTarget() {
  const key = \`\${process.platform}-\${process.arch}\`;
  const target = TARGETS[key];
  if (!target) {
    fail(
      \`unsupported platform/arch: \${process.platform}/\${process.arch}. Install from \${REPOSITORY_URL}/releases instead.\`,
    );
  }
  return target;
}

async function sha256(filePath) {
  return new Promise((resolve, reject) => {
    const hash = createHash("sha256");
    const stream = fs.createReadStream(filePath);

    stream.on("data", (chunk) => hash.update(chunk));
    stream.on("end", () => resolve(hash.digest("hex")));
    stream.on("error", reject);
  });
}

function parseChecksums(rawManifest) {
  const manifest = new Map();
  for (const line of rawManifest.split(/\\r?\\n/)) {
    const trimmed = line.trim();
    if (!trimmed) {
      continue;
    }

    const match = trimmed.match(/^([a-f0-9]{64})\\s+\\*?(.+)$/i);
    if (!match) {
      throw new Error(\`invalid checksum entry: \${trimmed}\`);
    }
    manifest.set(match[2], match[1]);
  }
  return manifest;
}

function runCommand(command, args) {
  const result = spawnSync(command, args, { stdio: "inherit" });
  if (result.error) {
    throw result.error;
  }
  if (result.status !== 0) {
    throw new Error(\`command failed (\${result.status}): \${command} \${args.join(" ")}\`);
  }
}

function extractArchive(archivePath, destination) {
  if (archivePath.endsWith(".tar.gz")) {
    runCommand("tar", ["-xzf", archivePath, "-C", destination]);
    return;
  }

  if (archivePath.endsWith(".zip")) {
    const escapedArchivePath = archivePath.replace(/'/g, "''");
    const escapedDestination = destination.replace(/'/g, "''");
    runCommand(
      "powershell.exe",
      [
        "-NoProfile",
        "-NonInteractive",
        "-Command",
        \`Expand-Archive -LiteralPath '\${escapedArchivePath}' -DestinationPath '\${escapedDestination}' -Force\`,
      ],
    );
    return;
  }

  throw new Error(\`unsupported archive format: \${archivePath}\`);
}

async function findFile(rootDir, fileName) {
  const entries = await fsp.readdir(rootDir, { withFileTypes: true });
  for (const entry of entries) {
    const fullPath = path.join(rootDir, entry.name);
    if (entry.isFile() && entry.name === fileName) {
      return fullPath;
    }
    if (entry.isDirectory()) {
      const nested = await findFile(fullPath, fileName);
      if (nested) {
        return nested;
      }
    }
  }
  return null;
}

async function installBinary({ target }) {
  const packageRoot = path.resolve(__dirname, "..");
  const binDir = path.join(packageRoot, "bin");
  const distsDir = path.join(packageRoot, "dists");
  const tempDir = await fsp.mkdtemp(path.join(os.tmpdir(), "wit-npm-"));
  const archivePath = path.join(distsDir, target.artifact);
  const checksumsPath = path.join(distsDir, CHECKSUM_FILE);
  const extractDir = path.join(tempDir, "extract");

  try {
    log(\`installing \${target.artifact} for \${process.platform}/\${process.arch}\`);
    await fsp.access(archivePath);

    const checksums = parseChecksums(await fsp.readFile(checksumsPath, "utf8"));
    const expectedChecksum = checksums.get(target.artifact);
    if (!expectedChecksum) {
      throw new Error(\`checksum entry missing for \${target.artifact}\`);
    }

    const actualChecksum = await sha256(archivePath);
    if (actualChecksum !== expectedChecksum) {
      throw new Error(\`checksum mismatch for \${target.artifact}\`);
    }

    await fsp.mkdir(extractDir, { recursive: true });
    extractArchive(archivePath, extractDir);

    await fsp.mkdir(binDir, { recursive: true });
    for (const binaryFile of target.binaryFiles || [target.binaryFile]) {
      const binaryPath = path.join(binDir, binaryFile);
      const sourceBinary =
        (await findFile(extractDir, binaryFile)) || path.join(extractDir, binaryFile);
      await fsp.access(sourceBinary);
      await fsp.rm(binaryPath, { force: true });
      await fsp.copyFile(sourceBinary, binaryPath);
      if (!binaryPath.endsWith(".exe")) {
        await fsp.chmod(binaryPath, 0o755);
      }
    }

    log(\`installed \${(target.binaryFiles || [target.binaryFile]).join(", ")} \${packageJson.version}\`);
  } finally {
    await fsp.rm(tempDir, { recursive: true, force: true });
  }
}

async function main() {
  const target = resolveTarget();
  await installBinary({ target });
}

main().catch((error) => {
  fail(error instanceof Error ? error.message : String(error));
});
`;
}

async function writePackage({ artifactsDir, outputDir, version, config, targets }) {
  const packageDir = path.join(outputDir, "package");
  const binDir = path.join(packageDir, "bin");
  const distsDir = path.join(packageDir, "dists");
  const scriptsDir = path.join(packageDir, "scripts");
  await ensureDir(binDir);
  await ensureDir(distsDir);
  await ensureDir(scriptsDir);

  const launcherConfigs = [
    {
      binaryName: config.binaryName,
      binaryFileKey: "binaryFile",
      relativePath: path.posix.join("bin", `${config.binaryName}.js`),
    },
  ];
  if (config.mcpBinaryName) {
    launcherConfigs.push({
      binaryName: config.mcpBinaryName,
      binaryFileKey: "mcpBinaryFile",
      relativePath: path.posix.join("bin", `${config.mcpBinaryName}.js`),
    });
  }
  const installerRelativePath = path.posix.join("scripts", "postinstall.js");

  const packageJson = {
    name: config.packageName,
    version,
    description: config.description,
    license: "UNLICENSED",
    repository: {
      type: "git",
      url: `${config.repositoryUrl}.git`,
    },
    homepage: `${config.repositoryUrl}#readme`,
    bugs: {
      url: `${config.repositoryUrl}/issues`,
    },
    bin: Object.fromEntries(
      launcherConfigs.map((launcher) => [launcher.binaryName, launcher.relativePath]),
    ),
    scripts: {
      postinstall: "node scripts/postinstall.js",
    },
    engines: {
      node: ">=18",
    },
    files: [
      ...launcherConfigs.map((launcher) => launcher.relativePath),
      installerRelativePath,
      "README.md",
      "dists/*",
    ],
  };

  await fs.writeFile(
    path.join(packageDir, "package.json"),
    `${JSON.stringify(packageJson, null, 2)}\n`,
    "utf8",
  );

  for (const launcherConfig of launcherConfigs) {
    const launcher = buildLauncher(
      { ...config, binaryName: launcherConfig.binaryName },
      targets,
      launcherConfig.binaryFileKey,
    );
    const launcherPath = path.join(binDir, `${launcherConfig.binaryName}.js`);
    await fs.writeFile(launcherPath, launcher, "utf8");
    await fs.chmod(launcherPath, 0o755);
  }

  const installer = buildInstaller(config, targets);
  const installerPath = path.join(scriptsDir, "postinstall.js");
  await fs.writeFile(installerPath, installer, "utf8");
  await fs.chmod(installerPath, 0o755);

  const rootReadmePath = path.join(repoRoot, "README.md");
  await fs.copyFile(rootReadmePath, path.join(packageDir, "README.md"));

  for (const target of targets) {
    await fs.copyFile(
      path.join(artifactsDir, target.artifact),
      path.join(distsDir, target.artifact),
    );
  }
  await fs.copyFile(path.join(artifactsDir, CHECKSUM_FILE), path.join(distsDir, CHECKSUM_FILE));
}

async function writeManifest({ outputDir, version, config }) {
  const manifest = {
    version,
    packageName: config.packageName,
    packageDir: path.join(outputDir, "package"),
  };

  await fs.writeFile(
    path.join(outputDir, "manifest.json"),
    `${JSON.stringify(manifest, null, 2)}\n`,
    "utf8",
  );
}

async function main() {
  const args = parseArgs(process.argv.slice(2));
  const artifactsDir = path.resolve(args.artifactsDir);
  const outputDir = path.resolve(args.outputDir);
  const config = await readTargetsConfig();

  await assertArtifactsExist(artifactsDir, config.targets);
  await fs.rm(outputDir, { recursive: true, force: true });
  await ensureDir(outputDir);

  await writePackage({
    artifactsDir,
    outputDir,
    version: args.version,
    config,
    targets: config.targets,
  });

  await writeManifest({
    outputDir,
    version: args.version,
    config,
  });
}

main().catch((error) => {
  fail(error instanceof Error ? error.message : String(error));
});
