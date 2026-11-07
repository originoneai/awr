'use strict';

const {spawn} = require('node:child_process');
const path = require('node:path');

exports.run = function run(command) {
  const targets = {
    'darwin-arm64': 'darwin-arm64',
    'darwin-x64': 'darwin-x64',
    'linux-x64': 'linux-x64-gnu',
    'linux-arm64': 'linux-arm64-gnu',
    'win32-x64': 'win32-x64',
  };
  const target = targets[`${process.platform}-${process.arch}`];
  function fail(message) {
    process.stderr.write(`AWR: ${message}\n`);
    process.exitCode = 1;
  }
  if (!target) return fail(`unsupported platform: ${process.platform}/${process.arch}`);
  if (process.platform === 'linux') {
    const glibc = process.report.getReport().header.glibcVersionRuntime;
    const [major, minor] = (glibc || '0.0').split('.').map(Number);
    if (major < 2 || (major === 2 && minor < 39)) {
      return fail('this Linux distribution requires glibc 2.39 or newer (Ubuntu 24.04 baseline).');
    }
  }
  const packageName = `@originoneai/awr-${target}`;
  let root;
  try {
    root = path.dirname(require.resolve(`${packageName}/package.json`));
  } catch {
    return fail(`missing ${packageName}; reinstall with optional dependencies enabled.`);
  }
  const executable = path.join(root, 'bin', command + (process.platform === 'win32' ? '.exe' : ''));
  const child = spawn(executable, process.argv.slice(2), {stdio: 'inherit', windowsHide: false});
  const forward = signal => { if (child.pid) child.kill(signal); };
  process.on('SIGINT', forward);
  process.on('SIGTERM', forward);
  child.once('error', error => {
    process.removeListener('SIGINT', forward);
    process.removeListener('SIGTERM', forward);
    fail(`could not launch ${command}: ${error.message}`);
  });
  child.once('exit', (code, signal) => {
    process.removeListener('SIGINT', forward);
    process.removeListener('SIGTERM', forward);
    if (signal) process.kill(process.pid, signal);
    else process.exitCode = code ?? 1;
  });
};
