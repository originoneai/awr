'use strict';

/**
 * Locate and run the awr executable for command-line tools (the Project map export). Arguments are passed as an array,
 * never through a shell. The Inspector server resolves awr the same way, from PATH.
 */

const fs = require('fs');
const path = require('path');
const { spawn } = require('child_process');

/** Entry point of an npm `.cmd` wrapper (`"node" "<entry>" %*`), or null. */
function parseCmdEntryPoint(cmdPath) {
  try {
    const match = fs.readFileSync(cmdPath, 'utf8').match(/"[^"]+"\s+"([^"]+)"\s+%\*/);
    if (match) {
      const entry = match[1].replace(/%dp0%/g, path.dirname(cmdPath));
      if (fs.existsSync(entry)) return entry;
    }
  } catch (_) { /* fall through */ }
  return null;
}

/** {exe, needsNode, entryPoint?}; `exe` is null when nothing was found. `override` names an explicit executable. */
function resolveAwr({ override, env = process.env, platform = process.platform } = {}) {
  if (override) {
    const file = path.resolve(override);
    return fs.existsSync(file) ? { exe: file, needsNode: false } : { exe: null, needsNode: false };
  }
  const sep = platform === 'win32' ? ';' : ':';
  const exts = platform === 'win32' ? (env.PATHEXT || '.COM;.EXE;.BAT;.CMD').split(';').map((e) => e.toUpperCase()) : [''];
  for (const dir of (env.PATH || '').split(sep)) {
    for (const ext of exts) {
      const candidate = path.join(dir, `awr${ext}`);
      let stat = null;
      try {
        stat = fs.statSync(candidate, { throwIfNoEntry: false });
      } catch (_) { /* unreadable PATH entry */ }
      if (!stat || !stat.isFile()) continue;
      const kind = path.extname(candidate).toUpperCase();
      if (kind === '.EXE' || kind === '') return { exe: candidate, needsNode: false };
      if (kind === '.CMD' || kind === '.BAT') {
        const entryPoint = parseCmdEntryPoint(candidate);
        if (entryPoint) return { exe: candidate, needsNode: true, entryPoint };
      }
    }
  }
  return { exe: null, needsNode: false };
}

/** Run one command; resolves {code, stdout, stderr}. Output beyond `maxBytes` or a run beyond `timeoutMs` rejects. */
function runAwr(resolved, project, args, { timeoutMs = 120000, maxBytes = 64 * 1024 * 1024 } = {}) {
  return new Promise((resolve, reject) => {
    const argv = ['--project', project, '--json', ...args];
    const cmd = resolved.needsNode ? process.execPath : resolved.exe;
    const child = spawn(cmd, resolved.needsNode ? [resolved.entryPoint, ...argv] : argv, { stdio: 'pipe', windowsHide: true });
    const out = [];
    const err = [];
    let bytes = 0;
    let settled = false;
    const finish = (fn, value) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      fn(value);
    };
    const timer = setTimeout(() => {
      child.kill('SIGKILL');
      finish(reject, Object.assign(new Error(`awr ${args.join(' ')} timed out`), { code: 'Timeout' }));
    }, timeoutMs);
    child.stdout.on('data', (chunk) => {
      bytes += chunk.length;
      if (bytes > maxBytes) {
        child.kill('SIGKILL');
        finish(reject, Object.assign(new Error('awr output is too large'), { code: 'OutputTooLarge' }));
        return;
      }
      out.push(chunk);
    });
    child.stderr.on('data', (chunk) => err.push(chunk));
    child.on('error', (e) => finish(reject, Object.assign(new Error(e.message), { code: 'SpawnFailed' })));
    child.on('close', (code) => finish(resolve, { code, stdout: Buffer.concat(out).toString('utf8'), stderr: Buffer.concat(err).toString('utf8') }));
  });
}

module.exports = { resolveAwr, runAwr, parseCmdEntryPoint };
