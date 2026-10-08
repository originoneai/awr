'use strict';

/**
 * A rig for tests that need the real bridge (server.js) or the export script in front of the fake awr: a temporary
 * directory holding the snapshot the fake answers from, an `awr` launcher on PATH and a log of every argument list.
 */

const { spawn } = require('node:child_process');
const fs = require('node:fs');
const net = require('node:net');
const os = require('node:os');
const path = require('node:path');

const ROOT = path.join(__dirname, '..', '..', '..');
const STUB = path.join(__dirname, 'stub-awr-map.js');

function freePort() {
  return new Promise((resolve, reject) => {
    const probe = net.createServer();
    probe.once('error', reject);
    probe.listen(0, '127.0.0.1', () => {
      const { port } = probe.address();
      probe.close(() => resolve(port));
    });
  });
}

function createRig(snapshot, files = {}) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'awr-map-rig-'));
  const rig = {
    dir,
    project: path.join(dir, 'project'),
    snapshotFile: path.join(dir, 'snapshot.json'),
    argvLog: path.join(dir, 'argv.jsonl'),
    bin: path.join(dir, 'bin'),
    launcher: null,
  };
  fs.mkdirSync(rig.project);
  fs.mkdirSync(rig.bin);
  fs.writeFileSync(rig.snapshotFile, JSON.stringify(snapshot));
  for (const [name, value] of Object.entries(files)) fs.writeFileSync(path.join(dir, name), typeof value === 'string' ? value : JSON.stringify(value));
  if (process.platform === 'win32') {
    rig.launcher = path.join(rig.bin, 'awr.cmd');
    fs.writeFileSync(rig.launcher, `"${process.execPath}" "${STUB}" %*\r\n`);
  } else {
    rig.launcher = path.join(rig.bin, 'awr');
    fs.writeFileSync(rig.launcher, `#!/bin/sh\nexec "${process.execPath}" "${STUB}" "$@"\n`);
    fs.chmodSync(rig.launcher, 0o755);
  }
  rig.file = (name) => path.join(dir, name);
  rig.env = (extra = {}) => ({ ...process.env, FAKE_SNAPSHOT: rig.snapshotFile, FAKE_ARGV_OUT: rig.argvLog, ...extra });
  /** Every argument list awr was started with, as the fake saw it (including --project and --json). */
  rig.calls = () => (fs.existsSync(rig.argvLog) ? fs.readFileSync(rig.argvLog, 'utf8').trim().split('\n').filter(Boolean).map((l) => JSON.parse(l)) : []);
  rig.clearCalls = () => fs.rmSync(rig.argvLog, { force: true });

  /** Start server.js on a free port with the fake awr on PATH. */
  rig.startBridge = async (extraArgs = [], env = {}) => {
    const port = await freePort();
    const sep = process.platform === 'win32' ? ';' : ':';
    const child = spawn(process.execPath, ['server.js', '--no-open', '--port', String(port), '--project', rig.project, ...extraArgs], {
      cwd: ROOT,
      env: { ...rig.env(env), PATH: `${rig.bin}${sep}${process.env.PATH}` },
      stdio: ['ignore', 'pipe', 'pipe'],
    });
    let output = '';
    await new Promise((resolve, reject) => {
      const timer = setTimeout(() => reject(new Error('bridge startup timed out')), 15000);
      child.stdout.on('data', (d) => { output += d; if (String(d).includes('started')) { clearTimeout(timer); resolve(); } });
      child.stderr.on('data', (d) => { output += d; });
      child.on('exit', (code) => { clearTimeout(timer); reject(new Error(`bridge exited with code ${code}: ${output}`)); });
    });
    return { port, base: `http://127.0.0.1:${port}`, stop: () => new Promise((r) => { child.on('exit', r); child.kill('SIGKILL'); }) };
  };
  rig.cleanup = () => fs.rmSync(dir, { recursive: true, force: true });
  return rig;
}

module.exports = { createRig, ROOT };
