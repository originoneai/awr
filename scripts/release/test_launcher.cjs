'use strict';

const assert = require('node:assert/strict');
const {EventEmitter} = require('node:events');
const fs = require('node:fs');
const path = require('node:path');
const test = require('node:test');
const vm = require('node:vm');

const source = fs.readFileSync(path.join(__dirname, '../../packaging/npm/bin/launcher.cjs'), 'utf8');

function launch(arch, command, missing = false) {
  const child = new EventEmitter();
  const process = new EventEmitter();
  const calls = {spawn: [], resolved: [], stderr: ''};
  Object.assign(process, {
    platform: 'darwin', arch,
    argv: ['node', command, '--project', '/tmp/项目 with spaces', '--json'],
    stderr: {write: value => { calls.stderr += value; }},
  });
  const requireMock = name => {
    if (name === 'node:path') return path;
    assert.equal(name, 'node:child_process');
    return {spawn: (...args) => { calls.spawn.push(args); return child; }};
  };
  requireMock.resolve = name => {
    calls.resolved.push(name);
    if (missing) throw new Error('MODULE_NOT_FOUND');
    return path.join('/native-packages', name);
  };
  const exports = {};
  vm.runInNewContext(source, {exports, process, require: requireMock});
  exports.run(command);
  return {calls, child, process};
}

for (const arch of ['arm64', 'x64']) {
  for (const command of ['awr', 'awr-mcp']) {
    test(`macOS ${arch} ${command} selects its native package and forwards arguments and exit status`, () => {
      const {calls, child, process} = launch(arch, command);
      assert.deepEqual(calls.resolved, [`@originoneai/awr-darwin-${arch}/package.json`]);
      assert.equal(calls.spawn.length, 1);
      assert.equal(calls.spawn[0][0], path.join('/native-packages', `@originoneai/awr-darwin-${arch}/bin/${command}`));
      assert.deepEqual(Array.from(calls.spawn[0][1]), ['--project', '/tmp/项目 with spaces', '--json']);
      assert.equal(calls.spawn[0][2].stdio, 'inherit');
      child.emit('exit', 7, null);
      assert.equal(process.exitCode, 7);
      assert.equal(process.listenerCount('SIGINT'), 0);
      assert.equal(process.listenerCount('SIGTERM'), 0);
      assert.equal(calls.stderr, '');
    });
  }
}

test('a missing Intel package reports how to reinstall without launching another architecture', () => {
  const {calls, process} = launch('x64', 'awr', true);
  assert.equal(calls.spawn.length, 0);
  assert.equal(process.exitCode, 1);
  assert.match(calls.stderr, /missing @originoneai\/awr-darwin-x64; reinstall with optional dependencies enabled/);
});

test('unsupported macOS architectures remain rejected', () => {
  const {calls, process} = launch('ia32', 'awr');
  assert.equal(calls.spawn.length, 0);
  assert.equal(calls.resolved.length, 0);
  assert.equal(process.exitCode, 1);
  assert.match(calls.stderr, /unsupported platform: darwin\/ia32/);
});
