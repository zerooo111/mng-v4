#!/usr/bin/env node

const { spawnSync } = require('child_process');
const fs = require('fs');
const path = require('path');

function main() {
  const { forceBuild, profile } = parseArgs(process.argv.slice(2));
  const repoRoot = path.resolve(__dirname, '..');
  const targetDir = path.join(repoRoot, 'target');
  const addonPath = path.join(targetDir, profile, 'rust_harness.node');
  const shouldAutoBuild =
    (process.env.CONTINUUM_HARNESS_RUST_AUTO_BUILD || 'true').toLowerCase() !==
    'false';

  let sharedLibPath = detectSharedLibraryPath(targetDir, profile);
  if (forceBuild || !sharedLibPath) {
    if (!shouldAutoBuild && !sharedLibPath) {
      throw new Error(
        `rust-harness shared library is missing under "${path.join(targetDir, profile)}" and auto-build is disabled`,
      );
    }
    if (forceBuild || shouldAutoBuild) {
      runCargoBuild(repoRoot, profile);
      sharedLibPath = detectSharedLibraryPath(targetDir, profile);
    }
  }

  if (!sharedLibPath) {
    throw new Error(
      `rust-harness shared library was not found under "${path.join(targetDir, profile)}"`,
    );
  }

  if (shouldRefreshAddon(sharedLibPath, addonPath)) {
    fs.copyFileSync(sharedLibPath, addonPath);
  }

  process.stdout.write(`${addonPath}\n`);
}

function parseArgs(args) {
  let forceBuild = false;
  let profile =
    (process.env.CONTINUUM_HARNESS_RUST_PROFILE || 'release').trim() || 'release';

  for (let i = 0; i < args.length; i += 1) {
    const arg = args[i];
    if (arg === '--build') {
      forceBuild = true;
      continue;
    }
    if (arg === '--release') {
      profile = 'release';
      continue;
    }
    if (arg === '--debug') {
      profile = 'debug';
      continue;
    }
    if (arg === '--profile') {
      const next = args[i + 1];
      if (!next) {
        throw new Error('missing value for --profile');
      }
      profile = next;
      i += 1;
      continue;
    }
    throw new Error(`unsupported argument "${arg}"`);
  }

  return { forceBuild, profile };
}

function runCargoBuild(repoRoot, profile) {
  const cargoArgs = ['build', '-p', 'rust-harness'];
  if (profile === 'release') {
    cargoArgs.push('--release');
  } else if (profile !== 'debug') {
    cargoArgs.push('--profile', profile);
  }

  const build = spawnSync('cargo', cargoArgs, {
    cwd: repoRoot,
    env: process.env,
    stdio: 'inherit',
  });
  if (build.status !== 0) {
    throw new Error(`cargo build failed with exit code ${build.status}`);
  }
}

function shouldRefreshAddon(sharedLibPath, addonPath) {
  if (!fs.existsSync(addonPath)) {
    return true;
  }
  try {
    const sharedStat = fs.statSync(sharedLibPath);
    const addonStat = fs.statSync(addonPath);
    return sharedStat.mtimeMs >= addonStat.mtimeMs;
  } catch {
    return true;
  }
}

function detectSharedLibraryPath(targetDir, profile) {
  const candidates =
    process.platform === 'win32'
      ? [path.join(targetDir, profile, 'rust_harness.dll')]
      : process.platform === 'darwin'
        ? [path.join(targetDir, profile, 'librust_harness.dylib')]
        : [path.join(targetDir, profile, 'librust_harness.so')];

  for (const candidate of candidates) {
    if (fs.existsSync(candidate)) {
      return candidate;
    }
  }
  return null;
}

try {
  main();
} catch (err) {
  const message = err instanceof Error ? err.message : `${err}`;
  process.stderr.write(`${message}\n`);
  process.exit(1);
}
