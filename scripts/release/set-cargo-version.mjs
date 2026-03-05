#!/usr/bin/env node

import fs from "node:fs/promises";
import path from "node:path";

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
    if (arg === "--cargo-toml") {
      args.cargoToml = argv[i + 1];
      i += 1;
      continue;
    }
    if (arg === "--cargo-lock") {
      args.cargoLock = argv[i + 1];
      i += 1;
      continue;
    }
    fail(`unknown argument: ${arg}`);
  }

  if (!args.version) {
    fail("missing required --version");
  }
  if (!args.cargoToml) {
    fail("missing required --cargo-toml");
  }

  if (!/^\d+\.\d+\.\d+$/.test(args.version)) {
    fail(`invalid semver version: ${args.version}`);
  }

  return args;
}

function packageSectionMetadata(contents) {
  const lines = contents.split(/\r?\n/);
  const packageStart = lines.findIndex((line) => line.trim() === "[package]");
  if (packageStart === -1) {
    fail("unable to locate [package] section in Cargo.toml");
  }

  let packageEnd = lines.length;
  for (let i = packageStart + 1; i < lines.length; i += 1) {
    const trimmed = lines[i].trim();
    if (trimmed.startsWith("[") && trimmed.endsWith("]")) {
      packageEnd = i;
      break;
    }
  }

  let nameLine = -1;
  let versionLine = -1;
  for (let i = packageStart + 1; i < packageEnd; i += 1) {
    if (nameLine === -1 && /^\s*name\s*=/.test(lines[i])) {
      nameLine = i;
    }
    if (/^\s*version\s*=/.test(lines[i])) {
      versionLine = i;
      break;
    }
  }

  if (nameLine === -1) {
    fail("unable to locate package name in [package] section");
  }
  if (versionLine === -1) {
    fail("unable to locate version in [package] section");
  }

  const nameMatch = lines[nameLine].match(/^\s*name\s*=\s*"([^"]+)"\s*$/);
  if (!nameMatch) {
    fail("unable to parse package name in [package] section");
  }

  return {
    lines,
    packageName: nameMatch[1],
    versionLine,
  };
}

function updatePackageVersion(contents, version) {
  const metadata = packageSectionMetadata(contents);
  const lines = metadata.lines;

  lines[metadata.versionLine] = `version = "${version}"`;
  return {
    updatedContents: `${lines.join("\n")}\n`,
    packageName: metadata.packageName,
  };
}

function updateLockVersion(cargoLockContents, packageName, version) {
  const lines = cargoLockContents.split(/\r?\n/);
  const candidates = [];

  for (let i = 0; i < lines.length; i += 1) {
    if (lines[i].trim() !== "[[package]]") {
      continue;
    }

    const blockStart = i;
    let blockEnd = lines.length;
    for (let j = i + 1; j < lines.length; j += 1) {
      if (lines[j].trim() === "[[package]]") {
        blockEnd = j;
        break;
      }
    }

    let nameLine = -1;
    let versionLine = -1;
    let hasSource = false;
    for (let j = blockStart + 1; j < blockEnd; j += 1) {
      const line = lines[j];
      if (nameLine === -1 && /^\s*name\s*=/.test(line)) {
        nameLine = j;
      }
      if (versionLine === -1 && /^\s*version\s*=/.test(line)) {
        versionLine = j;
      }
      if (/^\s*source\s*=/.test(line)) {
        hasSource = true;
      }
    }

    if (nameLine === -1 || versionLine === -1) {
      i = blockEnd - 1;
      continue;
    }

    const nameMatch = lines[nameLine].match(/^\s*name\s*=\s*"([^"]+)"\s*$/);
    if (nameMatch && nameMatch[1] === packageName) {
      candidates.push({ versionLine, hasSource });
    }

    i = blockEnd - 1;
  }

  if (candidates.length === 0) {
    fail(`unable to locate ${packageName} package block in Cargo.lock`);
  }

  const target =
    candidates.find((candidate) => candidate.hasSource === false) ?? candidates[0];
  lines[target.versionLine] = `version = "${version}"`;
  return `${lines.join("\n")}\n`;
}

async function main() {
  const args = parseArgs(process.argv.slice(2));
  const cargoTomlPath = path.resolve(args.cargoToml);
  const cargoToml = await fs.readFile(cargoTomlPath, "utf8");
  const { updatedContents, packageName } = updatePackageVersion(cargoToml, args.version);
  await fs.writeFile(cargoTomlPath, updatedContents, "utf8");

  const cargoLockPath = path.resolve(
    args.cargoLock ?? path.join(path.dirname(cargoTomlPath), "Cargo.lock"),
  );
  try {
    const cargoLock = await fs.readFile(cargoLockPath, "utf8");
    const updatedLock = updateLockVersion(cargoLock, packageName, args.version);
    await fs.writeFile(cargoLockPath, updatedLock, "utf8");
  } catch (error) {
    if (error && typeof error === "object" && "code" in error && error.code === "ENOENT") {
      return;
    }
    throw error;
  }
}

main().catch((error) => {
  fail(error instanceof Error ? error.message : String(error));
});
