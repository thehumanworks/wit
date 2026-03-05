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
    if (arg === "--tag") {
      args.tag = argv[i + 1];
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

  if (!args.tag) {
    fail("missing required --tag");
  }
  if (!args.cargoToml) {
    fail("missing required --cargo-toml");
  }
  return args;
}

function parsePackageVersion(cargoTomlContents) {
  const lines = cargoTomlContents.split(/\r?\n/);
  const packageStart = lines.findIndex((line) => line.trim() === "[package]");
  if (packageStart === -1) {
    fail("unable to locate [package] section in Cargo.toml");
  }

  let packageEnd = lines.length;
  for (let i = packageStart + 1; i < lines.length; i += 1) {
    if (lines[i].trim().startsWith("[") && lines[i].trim().endsWith("]")) {
      packageEnd = i;
      break;
    }
  }

  const section = lines.slice(packageStart, packageEnd).join("\n");
  const versionMatch = section.match(/^\s*version\s*=\s*"([^"]+)"\s*$/m);
  if (!versionMatch) {
    fail("unable to locate package version in Cargo.toml");
  }
  return versionMatch[1];
}

async function main() {
  const args = parseArgs(process.argv.slice(2));
  const cargoTomlPath = path.resolve(args.cargoToml);
  const cargoToml = await fs.readFile(cargoTomlPath, "utf8");
  const version = parsePackageVersion(cargoToml);
  const expectedTag = `v${version}`;

  if (args.tag !== expectedTag) {
    fail(`tag/version mismatch: got tag ${args.tag}, expected ${expectedTag}`);
  }

  process.stdout.write(version);
}

main().catch((error) => {
  fail(error instanceof Error ? error.message : String(error));
});
