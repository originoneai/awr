#!/usr/bin/env node
'use strict';

// A fake awr executable for the export tests. FAKE_SNAPSHOT names the snapshot file it answers from; FAKE_ARGV_OUT, when
// set, receives one JSON line with the arguments of every call; FAKE_OPTIONS holds createFakeCli options as JSON.
const fs = require('fs');
const { createFakeCli } = require('./fake-cli.js');

const argv = process.argv.slice(2);
if (process.env.FAKE_ARGV_OUT) fs.appendFileSync(process.env.FAKE_ARGV_OUT, `${JSON.stringify(argv)}\n`);
if (argv.includes('--version')) {
  process.stdout.write('awr 0.4.0-stub\n');
  process.exit(0);
}
const snapshot = JSON.parse(fs.readFileSync(process.env.FAKE_SNAPSHOT, 'utf8'));
const cli = createFakeCli(snapshot, process.env.FAKE_OPTIONS ? JSON.parse(process.env.FAKE_OPTIONS) : {});
const result = cli.run(argv);
process.stdout.write(result.stdout);
process.stderr.write(result.stderr);
process.exit(result.code);
