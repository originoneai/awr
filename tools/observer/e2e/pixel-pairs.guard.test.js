const assert = require('node:assert/strict');
const { readdirSync, readFileSync } = require('node:fs');
const path = require('node:path');
const test = require('node:test');

const pairDirectory = path.join(__dirname, 'pixel-pairs');
const geometrySkips = new Set(['__x', '__y', '__w', '__h', '__ax', '__ay']);

function forbiddenSkips(table, source) {
  const violations = [];
  for (const pair of table.pairs ?? []) {
    const [label, , , , options] = pair;
    for (const property of options?.skip ?? []) {
      if (!geometrySkips.has(property)) violations.push({ source, pair: label, property });
    }
  }
  return violations;
}

function readTable(name) {
  return JSON.parse(readFileSync(path.join(pairDirectory, name), 'utf8'));
}

test('committed pixel-pair tables never skip style comparisons', () => {
  const violations = readdirSync(pairDirectory)
    .filter((name) => name.endsWith('.json'))
    .sort()
    .flatMap((name) => forbiddenSkips(readTable(name), name));
  assert.deepEqual(violations, []);
});

test('a colour skip on the page subtitle is rejected without writing a bad fixture', () => {
  const synthetic = structuredClone(readTable('members.json'));
  const subtitle = synthetic.pairs.find(([label]) => label === 'page subtitle');
  assert.ok(subtitle, 'the passing sample has a page subtitle pair');
  subtitle[4] = { ...subtitle[4], skip: ['color'] };
  assert.deepEqual(forbiddenSkips(synthetic, 'synthetic.json'), [
    { source: 'synthetic.json', pair: 'page subtitle', property: 'color' },
  ]);
});
