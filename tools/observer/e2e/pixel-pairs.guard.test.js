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

function committedTables() {
  return readdirSync(pairDirectory).filter((name) => name.endsWith('.json')).sort();
}

test('committed pixel-pair tables never skip style comparisons', () => {
  const violations = committedTables().flatMap((name) => forbiddenSkips(readTable(name), name));
  assert.deepEqual(violations, []);
});

test('the guard really reads the committed tables and their pairs', () => {
  const names = committedTables();
  assert.ok(names.length >= 9, `expected the committed tables, found ${names.join(', ')}`);
  for (const name of names) {
    assert.ok((readTable(name).pairs ?? []).length > 0, `${name} has no pairs`);
  }
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

test('every style property is rejected on any pair and only the six geometry offsets may be skipped', () => {
  const styleProperties = [
    'color', 'font-size', 'font-weight', 'line-height', 'letter-spacing', 'font-family',
    'background-color', 'border-top-color', 'border-bottom-color', 'border-top-left-radius', 'box-shadow',
    'padding-top', 'padding-left', 'margin-top', 'margin-bottom', 'stroke-width', 'width', 'height', 'display',
  ];
  for (const property of styleProperties) {
    const table = {
      pairs: [
        ['first', '.a', '.b', ['TEXT'], { skip: [property] }],
        ['middle', '.a', '.b', ['BOX']],
        ['last', '.a', '.b', ['BOX'], { noPosition: true, skip: [property] }],
      ],
    };
    assert.deepEqual(forbiddenSkips(table, 't.json'), [
      { source: 't.json', pair: 'first', property },
      { source: 't.json', pair: 'last', property },
    ], property);
  }
  for (const property of ['__x', '__y', '__w', '__h', '__ax', '__ay']) {
    assert.deepEqual(forbiddenSkips({ pairs: [['p', '.a', '.b', ['BOX'], { skip: [property] }]] }, 't.json'), [], property);
  }
});
