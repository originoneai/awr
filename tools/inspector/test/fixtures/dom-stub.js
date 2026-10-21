/**
 * A minimal DOM stub.
 *
 * Runs the actual app.js detail-rendering path in Node, without implementing
 * the full DOM specification; only the methods used by that path are included.
 *
 * Usage: call install() before require('../public/app.js').
 */

'use strict';

class StubElement {
  constructor(tag) {
    this.tagName = String(tag || 'div').toUpperCase();
    this.className = '';
    this.children = [];
    this.style = {};
    this.dataset = {};
    this.attributes = {};
    this.listeners = {};
    this.classList = {
      add() {},
      remove() {},
      toggle() {},
      contains() { return false; },
    };
    this.hidden = false;
    this._value = '';
    this._scrollTop = 0;
    this.clientHeight = 0;
    this.own = ''; // Own text, excluding child nodes.
  }

  get scrollHeight() {
    // Approximate block layout so clearing a scroll container exercises the
    // browser behavior relevant to rendering tests.
    return Math.max(this.clientHeight, this.children.length * 40 + (this.own ? 20 : 0));
  }

  get scrollTop() {
    return this._scrollTop;
  }

  set scrollTop(value) {
    const maximum = Math.max(0, this.scrollHeight - this.clientHeight);
    this._scrollTop = Math.max(0, Math.min(Number(value) || 0, maximum));
  }

  get value() {
    return this._value;
  }

  set value(value) {
    value = String(value);
    this._value = this.tagName === 'SELECT' && !this.children.some(c => c.value === value)
      ? '' : value;
  }

  set textContent(v) {
    this.own = v == null ? '' : String(v);
    this.children = [];
    this.scrollTop = this._scrollTop;
    if (this.tagName === 'SELECT') this._value = '';
  }

  get textContent() {
    return this.own + this.children.map((c) => c.textContent).join('');
  }

  get innerText() {
    return this.textContent;
  }

  get firstChild() {
    return this.children.length ? this.children[0] : null;
  }

  appendChild(child) {
    this.children.push(child);
    if (this.tagName === 'SELECT' && this.children.length === 1) this._value = child.value;
    return child;
  }

  removeChild(child) {
    this.children = this.children.filter((c) => c !== child);
    this.scrollTop = this._scrollTop;
    if (this.tagName === 'SELECT' && child.value === this._value) this._value = this.children[0]?.value || '';
    return child;
  }

  setAttribute(name, value) {
    this.attributes[name] = String(value);
  }

  getAttribute(name) {
    return Object.prototype.hasOwnProperty.call(this.attributes, name) ? this.attributes[name] : null;
  }

  addEventListener(type, fn) {
    (this.listeners[type] = this.listeners[type] || []).push(fn);
  }

  click() {
    return Promise.all((this.listeners.click || []).map(fn => fn({ target: this })));
  }

  querySelectorAll() {
    return [];
  }

  querySelector() {
    return null;
  }

  /** Recursively find the first matching descendant, such as a test button. */
  find(predicate) {
    for (const child of this.children) {
      if (predicate(child)) return child;
      const hit = child.find(predicate);
      if (hit) return hit;
    }
    return null;
  }
}

/** Element IDs requested by app.js through getElementById. */
const IDS = [
  'workDetail', 'detailId', 'detailStatus', 'rawWorkBody', 'rawWorkBody',
  'fWork', 'fGoal', 'fBudget', 'fIntent', 'cliMirror',
  'view-overview', 'view-work', 'view-context', 'view-sources',
  'workRows', 'workFilters', 'workSub', 'workEmpty',
];

function install() {
  const byId = new Map();
  for (const id of IDS) byId.set(id, new StubElement(id === 'fWork' ? 'select' : 'div'));
  // The persistent detail host is the scroll container in the Inspector UI.
  byId.set('teamDetail', new StubElement('div'));
  byId.get('teamDetail').clientHeight = 120;

  // Explanatory paragraphs selected by app.js with [data-note="..."].
  const bySelector = new Map();
  for (const note of ['contextChart', 'queues', 'checkpoints', 'mcp', 'criteria', 'completeness', 'pending', 'sessions', 'events']) {
    bySelector.set(`[data-note="${note}"]`, new StubElement('p'));
  }

  const document = {
    getElementById: (id) => {
      if (!byId.has(id)) byId.set(id, new StubElement('div'));
      return byId.get(id);
    },
    createElement: (tag) => new StubElement(tag),
    createElementNS: (_namespace, tag) => new StubElement(tag),
    createTextNode: (text) => {
      const node = new StubElement('#text');
      node.textContent = text;
      return node;
    },
    querySelectorAll: () => [],
    querySelector: (sel) => bySelector.get(sel) || null,
    addEventListener: () => {},
    documentElement: new StubElement('html'),
  };

  global.document = document;
  global.window = {
    AWR_DEMO: undefined,
    scrollTo: () => {},
    matchMedia: () => ({ matches: false }),
    confirm: () => true,
    addEventListener: () => {},
  };
  global.history = { replaceState: () => {} };
  global.location = { hash: '' };
  global.localStorage = {
    getItem: () => null,
    setItem: () => {},
    removeItem: () => {},
  };
  global.sessionStorage = global.localStorage;
  // Node exposes a read-only navigator; replace it with defineProperty.
  Object.defineProperty(global, 'navigator', {
    value: { clipboard: null },
    configurable: true,
    writable: true,
  });

  return { document, byId, bySelector };
}

module.exports = { install, StubElement };
