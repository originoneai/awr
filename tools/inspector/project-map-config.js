'use strict';

/**
 * Node-side helpers for the Project map: loading the display configuration file and validating event cursors that the
 * bridge forwards to `awr event history`. Nothing here touches project sources.
 */

const fs = require('fs');
const path = require('path');
const model = require('./public/project-map/model.js');

const MAX_CONFIG_BYTES = 256 * 1024;

/** Read and validate a display configuration file. Returns {file, config}; throws a short, readable Error. */
function load(file) {
  const resolved = path.resolve(file);
  let text;
  try {
    const stat = fs.statSync(resolved);
    if (!stat.isFile()) throw new Error('not a file');
    if (stat.size > MAX_CONFIG_BYTES) throw new Error(`larger than ${MAX_CONFIG_BYTES} bytes`);
    text = fs.readFileSync(resolved, 'utf8');
  } catch (e) {
    const reasons = { ENOENT: 'no such file', EACCES: 'permission denied', EISDIR: 'it is a directory' };
    throw new Error(`cannot read ${path.basename(resolved)}: ${reasons[e.code] || (e.code ? e.code : e.message)}`);
  }
  let config;
  try {
    config = JSON.parse(text);
  } catch (e) {
    throw new Error(`${path.basename(resolved)} is not valid JSON: ${e.message}`);
  }
  // Lanes may be derived from the data later; here the file only has to be well formed.
  model.normalizeConfig(config, { nodes: [] });
  return { file: resolved, config };
}

const ID = /^[A-Za-z0-9_-]{1,64}$/;

/** The `next_cursor` object of an event history page, as the JSON text the browser echoes back; null when malformed. */
function parseEventCursor(text) {
  if (typeof text !== 'string' || text.length > 2000) return null;
  let value;
  try {
    value = JSON.parse(text);
  } catch (_) {
    return null;
  }
  if (!value || typeof value !== 'object' || Array.isArray(value)) return null;
  const keys = Object.keys(value).sort();
  if (keys.join(',') !== 'created_at,event_id,project_id,project_revision') return null;
  if (!ID.test(String(value.project_id)) || !ID.test(String(value.event_id))) return null;
  if (!Number.isSafeInteger(value.project_revision) || value.project_revision < 0) return null;
  if (!Number.isSafeInteger(value.created_at)) return null;
  return { project_id: String(value.project_id), project_revision: value.project_revision, created_at: value.created_at, event_id: String(value.event_id) };
}

module.exports = { load, parseEventCursor, MAX_CONFIG_BYTES };
