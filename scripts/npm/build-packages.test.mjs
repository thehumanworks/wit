import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import fs from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { test } from "node:test";

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);
const repoRoot = path.resolve(__dirname, "..", "..");
const builder = path.join(repoRoot, "scripts", "npm", "build-packages.mjs");

const LINUX_X64 = "wit-linux-x86_64.tar.gz";
const DARWIN_ARM64 = "wit-macos-aarch64.tar.gz";
const WINDOWS_X64 = "wit-windows-x86_64.zip";

async function writeTarGz(archivePath, files) {
  const staging = await fs.mkdtemp(path.join(os.tmpdir(), "wit-npm-tar-"));
  try {
    const names = [];
    for (const [name, contents] of Object.entries(files)) {
      await fs.writeFile(path.join(staging, name), contents);
      names.push(name);
    }
    const result = spawnSync("tar", ["-czf", archivePath, ...names], {
      cwd: staging,
      encoding: "utf8",
    });
    if (result.status !== 0) {
      throw new Error(result.stderr || `tar failed for ${archivePath}`);
    }
  } finally {
    await fs.rm(staging, { recursive: true, force: true });
  }
}

async function writeChecksums(artifactsDir, filenames) {
  const lines = [];
  for (const name of filenames) {
    const hash = createHash("sha256")
      .update(await fs.readFile(path.join(artifactsDir, name)))
      .digest("hex");
    lines.push(`${hash}  ${name}`);
  }
  await fs.writeFile(
    path.join(artifactsDir, "wit-checksums.txt"),
    lines.length > 0 ? `${lines.join("\n")}\n` : "",
    "utf8",
  );
}

async function writeSbom(artifactsDir) {
  await fs.writeFile(path.join(artifactsDir, "wit-sbom.spdx.json"), "{}\n", "utf8");
}

async function writeValidNativeArchive(artifactsDir, artifactName) {
  await writeTarGz(path.join(artifactsDir, artifactName), {
    wit: "wit\n",
    "wit-mcp": "wit-mcp\n",
  });
}

function runBuilder({ artifactsDir, outputDir, version = "0.1.37" }) {
  return spawnSync(
    process.execPath,
    [
      builder,
      "--version",
      version,
      "--artifacts-dir",
      artifactsDir,
      "--output-dir",
      outputDir,
    ],
    { encoding: "utf8", cwd: repoRoot },
  );
}

async function withTempDirs(fn) {
  const root = await fs.mkdtemp(path.join(os.tmpdir(), "wit-npm-pack-"));
  const artifactsDir = path.join(root, "artifacts");
  const outputDir = path.join(root, "output");
  await fs.mkdir(artifactsDir);
  try {
    return await fn({ artifactsDir, outputDir });
  } finally {
    await fs.rm(root, { recursive: true, force: true });
  }
}

test("missing native archive is skipped when others exist", async () => {
  await withTempDirs(async ({ artifactsDir, outputDir }) => {
    await writeValidNativeArchive(artifactsDir, LINUX_X64);
    await writeValidNativeArchive(artifactsDir, DARWIN_ARM64);
    await writeChecksums(artifactsDir, [LINUX_X64, DARWIN_ARM64]);
    await writeSbom(artifactsDir);

    const result = runBuilder({ artifactsDir, outputDir });
    assert.equal(result.status, 0, result.stderr || result.stdout);
    assert.match(result.stdout, /skipped missing artifact/);
    assert.doesNotMatch(result.stderr, /artifact missing for win32-x64/);

    const distsDir = path.join(outputDir, "package", "dists");
    const distFiles = await fs.readdir(distsDir);
    assert.ok(distFiles.includes(LINUX_X64));
    assert.ok(distFiles.includes(DARWIN_ARM64));
    assert.ok(!distFiles.includes(WINDOWS_X64));

    const installer = await fs.readFile(
      path.join(outputDir, "package", "scripts", "postinstall.js"),
      "utf8",
    );
    const launcher = await fs.readFile(
      path.join(outputDir, "package", "bin", "wit.js"),
      "utf8",
    );
    const mcpLauncher = await fs.readFile(
      path.join(outputDir, "package", "bin", "wit-mcp.js"),
      "utf8",
    );
    for (const generated of [installer, launcher, mcpLauncher]) {
      assert.doesNotMatch(generated, /win32-x64/);
      assert.doesNotMatch(generated, /wit-windows-x86_64\.zip/);
      assert.match(generated, /linux-x64/);
      assert.match(generated, /darwin-arm64/);
    }
  });
});

test("present archive missing its checksum still fails", async () => {
  await withTempDirs(async ({ artifactsDir, outputDir }) => {
    await writeValidNativeArchive(artifactsDir, LINUX_X64);
    await writeChecksums(artifactsDir, []);
    await writeSbom(artifactsDir);

    const result = runBuilder({ artifactsDir, outputDir });
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /checksum entry missing for wit-linux-x86_64\.tar\.gz/);
  });
});

test("present archive with the wrong file count still fails", async () => {
  await withTempDirs(async ({ artifactsDir, outputDir }) => {
    await writeTarGz(path.join(artifactsDir, LINUX_X64), { wit: "wit\n" });
    await writeChecksums(artifactsDir, [LINUX_X64]);
    await writeSbom(artifactsDir);

    const result = runBuilder({ artifactsDir, outputDir });
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /must contain exactly the two configured binaries/);
  });
});

test("zero native archives still fails", async () => {
  await withTempDirs(async ({ artifactsDir, outputDir }) => {
    await writeChecksums(artifactsDir, []);
    await writeSbom(artifactsDir);

    const result = runBuilder({ artifactsDir, outputDir });
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /no native archives present; nothing to publish/);
    assert.doesNotMatch(result.stderr, /artifact missing for win32-x64/);
  });
});
