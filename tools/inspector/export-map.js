#!/usr/bin/env node
'use strict';

/**
 * Export of the Project map: one self-contained HTML file (inline SVG, CSS and script, no external request) built only from
 * the official `awr --json` commands, or from a snapshot file saved earlier. It uses the same extractor, renderer and
 * interaction layer as the Inspector page, so the file shows and does what the page does: details panel, folding, highlight,
 * search, filter, zoom. With --static the file has no script at all (the diagrams only, for mail or print).
 *
 *   node export-map.js --project /path/to/project --out map.html [--lang en|zh-CN] [--config display.json]
 *                      [--cached] [--static] [--awr /path/to/awr] [--save-snapshot snapshot.json]
 *   node export-map.js --snapshot snapshot.json --out map.html
 */

const fs = require('fs');
const path = require('path');
const { resolveAwr, runAwr } = require('./awr-process');
const commands = require('./public/project-map/commands.js');
const snapshotLib = require('./public/project-map/snapshot.js');
const { AwrCallError, extractSnapshot } = require('./public/project-map/extract.js');
const pageLib = require('./public/project-map/page.js');
const configLoader = require('./project-map-config');

const catalogs = { en: require('./public/locales/en.js'), 'zh-CN': require('./public/locales/zh-CN.js') };
const MAP_DIR = path.join(__dirname, 'public', 'project-map');

/** The interaction layer to put into the file: module sources in load order, the stylesheet and the messages. */
function loadRuntime(lang) {
  const read = (name) => fs.readFileSync(path.join(MAP_DIR, name), 'utf8');
  return { sources: pageLib.RUNTIME_FILES.map(read), css: read('interact.css'), catalogs: pageLib.runtimeMessages(catalogs, lang) };
}

const USAGE = `Usage:
  node export-map.js --project <dir> [--out <file>] [options]
  node export-map.js --snapshot <file> [--out <file>] [options]

Options:
  --project <dir>          AWR project to read through the awr command (default: current directory)
  --snapshot <file>        Render a snapshot saved earlier instead of reading a project
  --out <file>             Output HTML file (default: awr-project-map.html; "-" writes to stdout)
  --lang <en|zh-CN>        Language of the page (default: en)
  --config <file>          Display configuration (JSON): lanes, dependency panels, thresholds
  --cached                 Read recorded state only (needs awr with work graph --cached)
  --static                 No script in the file: diagrams only, without the details panel and the other interactions
  --awr <path>             awr executable (default: awr on PATH)
  --save-snapshot <file>   Also write the snapshot JSON
  --now <time>             Time recorded as generated_at, YYYY-MM-DDTHH:MM:SSZ (default: now)
  --help                   Show this text`;

function parseArgs(argv) {
  const out = { project: '.', out: 'awr-project-map.html', lang: 'en', cached: false, static: false };
  const value = (i, flag) => {
    if (i + 1 >= argv.length) throw new Error(`${flag} needs a value`);
    return argv[i + 1];
  };
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (a === '--help' || a === '-h') out.help = true;
    else if (a === '--project') out.project = value(i++, a);
    else if (a === '--snapshot') out.snapshot = value(i++, a);
    else if (a === '--out') out.out = value(i++, a);
    else if (a === '--lang') out.lang = value(i++, a);
    else if (a === '--config') out.config = value(i++, a);
    else if (a === '--awr') out.awr = value(i++, a);
    else if (a === '--save-snapshot') out.saveSnapshot = value(i++, a);
    else if (a === '--now') out.now = value(i++, a);
    else if (a === '--cached') out.cached = true;
    else if (a === '--static') out.static = true;
    else throw new Error(`unknown option: ${a}`);
  }
  const lang = /^zh(?:-|$)/i.test(out.lang) ? 'zh-CN' : /^en(?:-|$)/i.test(out.lang) ? 'en' : null;
  if (!lang) throw new Error(`--lang must be en or zh-CN, not ${out.lang}`);
  out.lang = lang;
  if (out.now !== undefined && !/^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\dZ$/.test(out.now)) throw new Error('--now must look like 2026-01-15T00:00:00Z');
  return out;
}

/** Extractor `call` over the awr executable. Official JSON only; a nonzero exit with a report (doctor) is still a result. */
function makeCall(resolved, project) {
  return async (name, params) => {
    const result = await runAwr(resolved, project, commands.argvFor(name, params));
    const raw = (result.stdout.trim() ? result.stdout : result.stderr).trim();
    let value = null;
    try {
      value = JSON.parse(raw);
    } catch (_) {
      const i = raw.indexOf('{');
      if (i > 0) {
        try { value = JSON.parse(raw.slice(i)); } catch (__) { value = null; }
      }
    }
    const isError = value && typeof value === 'object' && 'code' in value && 'message' in value && !('ok' in value);
    if (value !== null && !isError) return value;
    throw new AwrCallError(isError ? value.code : 'NoOutput', isError ? String(value.message) : (raw.slice(0, 300) || 'empty output'), isError ? value.details || null : null);
  };
}

async function main(argv, io = { stdout: process.stdout, stderr: process.stderr }) {
  let args;
  try {
    args = parseArgs(argv);
  } catch (e) {
    io.stderr.write(`export-map: ${e.message}\n\n${USAGE}\n`);
    return 2;
  }
  if (args.help) {
    io.stdout.write(`${USAGE}\n`);
    return 0;
  }
  try {
    const config = args.config ? configLoader.load(args.config).config : undefined;
    let snapshot;
    if (args.snapshot) {
      snapshot = snapshotLib.validate(JSON.parse(fs.readFileSync(args.snapshot, 'utf8')));
    } else {
      const resolved = resolveAwr({ override: args.awr });
      if (!resolved.exe) throw new Error('the awr command was not found; install it, put it on PATH or pass --awr <path>');
      snapshot = await extractSnapshot({ call: makeCall(resolved, path.resolve(args.project)), config, now: args.now, cached: args.cached });
    }
    if (args.saveSnapshot) fs.writeFileSync(args.saveSnapshot, `${JSON.stringify(snapshot, null, 1)}\n`, 'utf8');
    const result = pageLib.renderProjectMap(snapshot, config, { t: pageLib.makeT(catalogs, args.lang), lang: args.lang, runtime: args.static ? null : loadRuntime(args.lang) });
    if (args.out === '-') io.stdout.write(result.document);
    else {
      fs.writeFileSync(args.out, result.document, 'utf8');
      io.stderr.write(`export-map: wrote ${args.out} (${snapshot.nodes.length} work items, revision ${snapshot.project.revision}, snapshot ${snapshot.fingerprint.slice(7, 19)})\n`);
    }
    return 0;
  } catch (e) {
    io.stderr.write(`export-map: ${e && e.message ? e.message : String(e)}\n`);
    return 2;
  }
}

if (require.main === module) {
  main(process.argv.slice(2)).then((code) => { process.exitCode = code; });
}

module.exports = { main, parseArgs, makeCall, loadRuntime, USAGE };
