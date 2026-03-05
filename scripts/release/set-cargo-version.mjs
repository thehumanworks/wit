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

function updatePackageVersion(contents, version) {
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

  let versionLine = -1;
  for (let i = packageStart + 1; i < packageEnd; i += 1) {
    if (/^\s*version\s*=/.test(lines[i])) {
      versionLine = i;
      break;
    }
  }

  if (versionLine === -1) {
    fail("unable to locate version in [package] section");
  }

  lines[versionLine] = `version = "${version}"`;
  return `${lines.join("\n")}\n`;
}

async function main() {
  const args = parseArgs(process.argv.slice(2));
  const cargoTomlPath = path.resolve(args.cargoToml);
  const cargoToml = await fs.readFile(cargoTomlPath, "utf8");
  const updated = updatePackageVersion(cargoToml, args.version);
  await fs.writeFile(cargoTomlPath, updated, "utf8");
}

main().catch((error) => {
  fail(error instanceof Error ? error.message : String(error));
});
