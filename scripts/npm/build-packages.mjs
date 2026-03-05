#!/usr/bin/env node

import { spawnSync } from "node:child_process";
import fs from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);
const repoRoot = path.resolve(__dirname, "..", "..");

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
    if (arg === "--npm-scope") {
      args.npmScope = argv[i + 1];
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

function normalizeScope(scope) {
  if (!scope) {
    return null;
  }

  const trimmed = scope.trim();
  if (trimmed.length === 0) {
    return null;
  }

  const withoutPrefix = trimmed.startsWith("@") ? trimmed.slice(1) : trimmed;
  if (withoutPrefix.length === 0) {
    fail(`invalid npm scope: ${scope}`);
  }

  if (
    withoutPrefix.includes("/") ||
    /\s/.test(withoutPrefix) ||
    /^[._]/.test(withoutPrefix) ||
    !/^[a-z0-9._-]+$/i.test(withoutPrefix)
  ) {
    fail(`invalid npm scope: ${scope}`);
  }

  return `@${withoutPrefix}`;
}

async function ensureDir(dirPath) {
  await fs.mkdir(dirPath, { recursive: true });
}

async function readTargetsConfig() {
  const configPath = path.join(repoRoot, "npm", "targets.json");
  const raw = await fs.readFile(configPath, "utf8");
  return JSON.parse(raw);
}

function replacePackageScope(packageName, npmScope) {
  const match = packageName.match(/^@[^/]+\/(.+)$/);
  if (!match) {
    fail(`expected scoped package name in targets config: ${packageName}`);
  }
  return `${npmScope}/${match[1]}`;
}

function applyScopeOverride(config, npmScope) {
  if (!npmScope) {
    return config;
  }

  return {
    ...config,
    basePackageName: replacePackageScope(config.basePackageName, npmScope),
    targets: config.targets.map((target) => ({
      ...target,
      packageName: replacePackageScope(target.packageName, npmScope),
    })),
  };
}

function runCommand(command, commandArgs, cwd) {
  const result = spawnSync(command, commandArgs, { cwd, stdio: "inherit" });
  if (result.error) {
    fail(`failed to run ${command}: ${result.error.message}`);
  }
  if (result.status !== 0) {
    fail(`command failed (${result.status}): ${command} ${commandArgs.join(" ")}`);
  }
}

async function findFile(rootDir, fileName) {
  const entries = await fs.readdir(rootDir, { withFileTypes: true });
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

async function extractBinary({
  artifactsDir,
  target,
  extractionDir,
}) {
  const artifactPath = path.join(artifactsDir, target.artifact);
  try {
    await fs.access(artifactPath);
  } catch {
    fail(`artifact missing for ${target.id}: ${artifactPath}`);
  }

  await ensureDir(extractionDir);
  if (target.artifact.endsWith(".tar.gz")) {
    runCommand("tar", ["-xzf", artifactPath, "-C", extractionDir], repoRoot);
  } else if (target.artifact.endsWith(".zip")) {
    runCommand("unzip", ["-oq", artifactPath, "-d", extractionDir], repoRoot);
  } else {
    fail(`unsupported artifact format for ${target.id}: ${target.artifact}`);
  }

  const directPath = path.join(extractionDir, target.binaryFile);
  try {
    await fs.access(directPath);
    return directPath;
  } catch {
    const foundPath = await findFile(extractionDir, target.binaryFile);
    if (!foundPath) {
      fail(`unable to locate extracted binary ${target.binaryFile} for ${target.id}`);
    }
    return foundPath;
  }
}

async function writePlatformPackage({
  outputDir,
  version,
  config,
  target,
  artifactsDir,
}) {
  const packageDir = path.join(outputDir, "platform", target.id);
  const binDir = path.join(packageDir, "bin");
  const extractionDir = path.join(outputDir, ".extract", target.id);

  await ensureDir(binDir);
  const sourceBinary = await extractBinary({ artifactsDir, target, extractionDir });
  const binaryRelativePath = path.join("bin", target.binaryFile);
  const targetBinaryPath = path.join(packageDir, binaryRelativePath);
  await fs.copyFile(sourceBinary, targetBinaryPath);

  if (!target.binaryFile.endsWith(".exe")) {
    await fs.chmod(targetBinaryPath, 0o755);
  }

  const packageJson = {
    name: target.packageName,
    version,
    description: `Prebuilt ${config.binaryName} binary for ${target.os}-${target.cpu}`,
    license: "UNLICENSED",
    repository: {
      type: "git",
      url: `${config.repositoryUrl}.git`,
    },
    os: [target.os],
    cpu: [target.cpu],
    files: [binaryRelativePath],
  };

  await fs.writeFile(
    path.join(packageDir, "package.json"),
    `${JSON.stringify(packageJson, null, 2)}\n`,
    "utf8",
  );

  const readme = [
    `# ${target.packageName}`,
    "",
    `Prebuilt ${config.binaryName} binary package for ${target.os}-${target.cpu}.`,
    "",
    `This package is published for platform resolution by \`${config.basePackageName}\`.`,
    "",
  ].join("\n");
  await fs.writeFile(path.join(packageDir, "README.md"), readme, "utf8");
}

function buildLauncher(targets) {
  const mapping = targets.reduce((acc, target) => {
    acc[`${target.os}-${target.cpu}`] = {
      packageName: target.packageName,
      binaryFile: target.binaryFile,
    };
    return acc;
  }, {});

  return ({ repositoryUrl, basePackageName }) => `#!/usr/bin/env node
"use strict";

const { spawnSync } = require("node:child_process");

const TARGETS = ${JSON.stringify(mapping, null, 2)};

function fail(message) {
  console.error(message);
  process.exit(1);
}

function resolveBinary() {
  const key = \`\${process.platform}-\${process.arch}\`;
  const target = TARGETS[key];
  if (!target) {
    fail(
      \`Unsupported platform/arch: \${process.platform}/\${process.arch}. Install from GitHub releases: ${repositoryUrl}/releases\`,
    );
  }

  try {
    return require.resolve(\`\${target.packageName}/bin/\${target.binaryFile}\`);
  } catch (error) {
    fail(
      \`Unable to resolve native binary package \${target.packageName}. Try reinstalling ${basePackageName}.\`,
    );
  }
}

const binaryPath = resolveBinary();
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

async function writeBasePackage({
  outputDir,
  version,
  config,
  targets,
}) {
  const baseDir = path.join(outputDir, "base");
  const baseBinDir = path.join(baseDir, "bin");
  await ensureDir(baseBinDir);

  const optionalDependencies = Object.fromEntries(
    targets.map((target) => [target.packageName, version]),
  );

  const packageJson = {
    name: config.basePackageName,
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
    bin: {
      [config.binaryName]: "bin/wit.js",
    },
    engines: {
      node: ">=18",
    },
    files: ["bin/wit.js", "README.md"],
    optionalDependencies,
  };

  await fs.writeFile(
    path.join(baseDir, "package.json"),
    `${JSON.stringify(packageJson, null, 2)}\n`,
    "utf8",
  );

  const launcher = buildLauncher(targets)({
    repositoryUrl: config.repositoryUrl,
    basePackageName: config.basePackageName,
  });
  const launcherPath = path.join(baseBinDir, "wit.js");
  await fs.writeFile(launcherPath, launcher, "utf8");
  await fs.chmod(launcherPath, 0o755);

  const rootReadmePath = path.join(repoRoot, "README.md");
  await fs.copyFile(rootReadmePath, path.join(baseDir, "README.md"));
}

async function writeManifest({ outputDir, version, config, targets }) {
  const manifest = {
    version,
    basePackageName: config.basePackageName,
    basePackageDir: path.join(outputDir, "base"),
    platformPackageDirs: targets.map((target) => path.join(outputDir, "platform", target.id)),
    platformPackages: targets.map((target) => ({
      id: target.id,
      packageName: target.packageName,
    })),
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
  const npmScope = normalizeScope(args.npmScope);
  const baseConfig = await readTargetsConfig();
  const config = applyScopeOverride(baseConfig, npmScope);
  const publishTargets = config.targets.filter((target) => target.publishToNpm);

  await fs.rm(outputDir, { recursive: true, force: true });
  await ensureDir(outputDir);

  for (const target of publishTargets) {
    await writePlatformPackage({
      outputDir,
      version: args.version,
      config,
      target,
      artifactsDir,
    });
  }

  await writeBasePackage({
    outputDir,
    version: args.version,
    config,
    targets: publishTargets,
  });

  await writeManifest({
    outputDir,
    version: args.version,
    config,
    targets: publishTargets,
  });
}

main().catch((error) => {
  fail(error instanceof Error ? error.message : String(error));
});
