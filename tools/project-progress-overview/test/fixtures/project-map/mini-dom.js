/**
 * A small DOM for testing the project-map interaction layer in Node, without a browser or any dependency.
 *
 * It parses the well-formed HTML and SVG the renderer produces and implements what interact.js, ui.js and the tests touch:
 * element tree, attributes, classList, innerHTML/textContent, a selector engine for the subset in use (tag, #id, .class,
 * [attr], [attr=value], compounds, the descendant combinator and comma lists), events with bubbling, focus with
 * focusin/focusout, radio groups, scroll geometry as plain numbers and a log of scrollIntoView calls.
 * It is not a browser: layout, CSS and default actions do not exist. Real-browser behaviour is checked separately.
 */

'use strict';

const VOID = new Set(['area', 'base', 'br', 'col', 'embed', 'hr', 'img', 'input', 'link', 'meta', 'source', 'track', 'wbr']);
const RAW = new Set(['script', 'style']);

const decode = (s) => s.replace(/&(#x[0-9a-f]+|#\d+|amp|lt|gt|quot|apos);/gi, (m, e) => {
  if (e[0] === '#') return String.fromCodePoint(e[1].toLowerCase() === 'x' ? parseInt(e.slice(2), 16) : parseInt(e.slice(1), 10));
  return { amp: '&', lt: '<', gt: '>', quot: '"', apos: "'" }[e.toLowerCase()];
});
const escapeText = (s) => s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
const escapeAttr = (s) => s.replace(/&/g, '&amp;').replace(/"/g, '&quot;').replace(/</g, '&lt;');

// ---------------------------------------------------------------------- selectors

function splitTop(selector, separator) {
  const parts = [];
  let depth = 0;
  let quote = null;
  let cur = '';
  for (let i = 0; i < selector.length; i++) {
    const c = selector[i];
    if (quote) {
      cur += c;
      if (c === '\\') cur += selector[++i];
      else if (c === quote) quote = null;
    } else if (c === '"' || c === "'") { quote = c; cur += c; }
    else if (c === '[') { depth++; cur += c; }
    else if (c === ']') { depth--; cur += c; }
    else if (depth === 0 && (separator === ',' ? c === ',' : /\s/.test(c))) {
      if (cur.trim()) parts.push(cur.trim());
      cur = '';
    } else cur += c;
  }
  if (cur.trim()) parts.push(cur.trim());
  return parts;
}

function parseCompound(s) {
  const out = { tag: null, id: null, classes: [], attrs: [] };
  let i = 0;
  const ident = () => {
    const m = /^[A-Za-z0-9_-]+/.exec(s.slice(i));
    if (!m) throw new Error(`unsupported selector: ${s}`);
    i += m[0].length;
    return m[0];
  };
  if (s[0] === '*') i = 1;
  else if (/[A-Za-z]/.test(s[0])) out.tag = ident().toLowerCase();
  while (i < s.length) {
    const c = s[i++];
    if (c === '#') out.id = ident();
    else if (c === '.') out.classes.push(ident());
    else if (c === '[') {
      const end = (() => {
        let q = null;
        for (let j = i; j < s.length; j++) {
          if (q) { if (s[j] === '\\') j++; else if (s[j] === q) q = null; } else if (s[j] === '"' || s[j] === "'") q = s[j]; else if (s[j] === ']') return j;
        }
        throw new Error(`unsupported selector: ${s}`);
      })();
      const inner = s.slice(i, end);
      i = end + 1;
      const eq = inner.indexOf('=');
      if (eq < 0) out.attrs.push({ name: inner.trim(), value: null });
      else {
        let value = inner.slice(eq + 1).trim();
        if (/^["']/.test(value)) value = value.slice(1, -1).replace(/\\(.)/g, '$1');
        out.attrs.push({ name: inner.slice(0, eq).trim(), value });
      }
    } else throw new Error(`unsupported selector: ${s}`);
  }
  return out;
}

const parseSelector = (selector) => splitTop(selector, ',').map((one) => splitTop(one, ' ').map(parseCompound));

function matchCompound(el, c) {
  if (!el.tagName) return false;
  if (c.tag && el.tagName.toLowerCase() !== c.tag) return false;
  if (c.id && el.getAttribute('id') !== c.id) return false;
  if (c.classes.length) {
    const have = (el.getAttribute('class') || '').split(/\s+/);
    if (!c.classes.every((k) => have.includes(k))) return false;
  }
  return c.attrs.every((a) => (a.value === null ? el.hasAttribute(a.name) : el.getAttribute(a.name) === a.value));
}

function matchChain(el, chain, at) {
  if (!matchCompound(el, chain[at])) return false;
  if (at === 0) return true;
  for (let p = el.parent; p && p.tagName; p = p.parent) if (matchChain(p, chain, at - 1)) return true;
  return false;
}

const matchesAny = (el, selectors) => selectors.some((chain) => matchChain(el, chain, chain.length - 1));

// ---------------------------------------------------------------------- nodes

class Text {
  constructor(data) { this.data = data; this.parent = null; }
  get textContent() { return this.data; }
}

class Element {
  constructor(tagName, doc) {
    this.tagName = tagName;
    this.ownerDocument = doc;
    this.parent = null;
    this.childNodes = [];
    this.attrs = new Map();
    this.listeners = new Map();
    this.scrollLeft = 0;
    this.scrollTop = 0;
    this.clientWidth = 0;
    this._value = null;
    this._checked = null;
    const declarations = new Map();
    this.style = { setProperty: (name, value) => declarations.set(name, String(value)), getPropertyValue: (name) => declarations.get(name) || '' };
    const self = this;
    this.classList = {
      contains: (name) => (self.getAttribute('class') || '').split(/\s+/).includes(name),
      add(...names) {
        const have = (self.getAttribute('class') || '').split(/\s+/).filter(Boolean);
        for (const n of names) if (!have.includes(n)) have.push(n);
        self.setAttribute('class', have.join(' '));
      },
      remove(...names) {
        const have = (self.getAttribute('class') || '').split(/\s+/).filter((n) => n && !names.includes(n));
        if (have.length) self.setAttribute('class', have.join(' '));
        else self.removeAttribute('class');
      },
      toggle(name, force) {
        const on = force === undefined ? !this.contains(name) : Boolean(force);
        if (on) this.add(name);
        else this.remove(name);
        return on;
      },
    };
  }

  get children() { return this.childNodes.filter((n) => n instanceof Element); }
  getAttribute(name) { return this.attrs.has(name) ? this.attrs.get(name) : null; }
  setAttribute(name, value) { this.attrs.set(name, String(value)); }
  removeAttribute(name) { this.attrs.delete(name); }
  hasAttribute(name) { return this.attrs.has(name); }
  get hidden() { return this.attrs.has('hidden'); }
  set hidden(value) { if (value) this.attrs.set('hidden', ''); else this.attrs.delete('hidden'); }
  get disabled() { return this.attrs.has('disabled'); }
  set disabled(value) { if (value) this.attrs.set('disabled', ''); else this.attrs.delete('disabled'); }
  get className() { return this.getAttribute('class') || ''; }
  set className(value) { this.setAttribute('class', value); }
  get id() { return this.getAttribute('id') || ''; }

  get value() { return this._value === null ? (this.getAttribute('value') || '') : this._value; }
  set value(v) { this._value = String(v); }

  get checked() { return this._checked === null ? this.hasAttribute('checked') : this._checked; }
  set checked(v) {
    this._checked = Boolean(v);
    if (v && this.getAttribute('type') === 'radio') {
      const name = this.getAttribute('name');
      for (const other of this.ownerDocument.documentElement.querySelectorAll('input')) {
        if (other !== this && other.getAttribute('name') === name && other.getAttribute('type') === 'radio') other._checked = false;
      }
    }
  }

  get textContent() { return this.childNodes.map((n) => n.textContent).join(''); }
  set textContent(value) {
    for (const c of this.childNodes) c.parent = null;
    this.childNodes = [];
    if (String(value) !== '') this.appendChild(new Text(String(value)));
  }

  get innerHTML() {
    return this.childNodes.map((n) => (n instanceof Text ? escapeText(n.data) : n.outerHTML)).join('');
  }
  set innerHTML(html) {
    for (const c of this.childNodes) c.parent = null;
    this.childNodes = [];
    parseInto(this, String(html), this.ownerDocument);
  }
  get outerHTML() {
    const attrs = [...this.attrs].map(([k, v]) => ` ${k}="${escapeAttr(v)}"`).join('');
    return VOID.has(this.tagName.toLowerCase()) ? `<${this.tagName}${attrs}>` : `<${this.tagName}${attrs}>${this.innerHTML}</${this.tagName}>`;
  }

  appendChild(child) {
    if (child.parent) child.parent.childNodes.splice(child.parent.childNodes.indexOf(child), 1);
    child.parent = this;
    this.childNodes.push(child);
    return child;
  }

  contains(node) {
    for (let n = node; n; n = n.parent) if (n === this) return true;
    return false;
  }

  * descendants() {
    for (const c of this.childNodes) {
      if (c instanceof Element) {
        yield c;
        yield* c.descendants();
      }
    }
  }

  querySelectorAll(selector) {
    const selectors = parseSelector(selector);
    return [...this.descendants()].filter((el) => matchesAny(el, selectors));
  }
  querySelector(selector) { return this.querySelectorAll(selector)[0] || null; }
  matches(selector) { return matchesAny(this, parseSelector(selector)); }
  closest(selector) {
    const selectors = parseSelector(selector);
    for (let el = this; el && el.tagName; el = el.parent) if (matchesAny(el, selectors)) return el;
    return null;
  }

  addEventListener(type, fn, options) {
    if (!this.listeners.has(type)) this.listeners.set(type, []);
    this.listeners.get(type).push({ fn, options });
  }
  removeEventListener(type, fn) {
    const list = this.listeners.get(type) || [];
    const at = list.findIndex((l) => l.fn === fn);
    if (at >= 0) list.splice(at, 1);
  }
  listenerCount(type) { return (this.listeners.get(type) || []).length; }

  dispatchEvent(event) {
    event.target = event.target || this;
    const path = [];
    for (let n = this; n; n = n.parent) path.push(n);
    for (const node of event.bubbles === false ? [this] : path) {
      event.currentTarget = node;
      for (const l of [...(node.listeners.get(event.type) || [])]) l.fn(event);
      if (event.cancelBubble) break;
    }
    return !event.defaultPrevented;
  }

  focus() { this.ownerDocument.setActive(this); }
  blur() { if (this.ownerDocument.activeElement === this) this.ownerDocument.setActive(null); }
  click() { return this.dispatchEvent(makeEvent('click')); }
  scrollIntoView(options) { this.ownerDocument.scrolled.push({ el: this, options }); }
  getBoundingClientRect() { return { left: 0, top: 0, width: this.clientWidth, height: 0 }; }
}

class Document extends Element {
  constructor() {
    super('#document', null);
    this.ownerDocument = this;
    this.documentElement = new Element('html', this);
    this.body = new Element('body', this);
    this.documentElement.appendChild(this.body);
    this.documentElement.parent = this;
    this.childNodes = [this.documentElement];
    this.activeElement = null;
    this.scrolled = [];
  }
  createElement(tag) { return new Element(tag, this); }
  getElementById(id) { return this.documentElement.querySelector(`#${id}`); }
  setActive(el) {
    const old = this.activeElement;
    if (old === el) return;
    this.activeElement = el;
    if (old) old.dispatchEvent(makeEvent('focusout', { relatedTarget: el }));
    if (el) el.dispatchEvent(makeEvent('focusin', { relatedTarget: old }));
  }
  get textContent() { return this.documentElement.textContent; }
}

function makeEvent(type, props = {}) {
  return {
    type, bubbles: true, defaultPrevented: false, cancelBubble: false, target: null, currentTarget: null,
    preventDefault() { this.defaultPrevented = true; },
    stopPropagation() { this.cancelBubble = true; },
    ...props,
  };
}

// ---------------------------------------------------------------------- parser

function parseInto(parent, html, doc) {
  const stack = [parent];
  const add = (node) => stack[stack.length - 1].appendChild(node);
  let i = 0;
  while (i < html.length) {
    if (html.startsWith('<!--', i)) {
      const end = html.indexOf('-->', i + 4);
      i = end < 0 ? html.length : end + 3;
    } else if (html.startsWith('<!', i)) {
      i = html.indexOf('>', i) + 1;
    } else if (html[i] === '<' && html[i + 1] === '/') {
      const end = html.indexOf('>', i);
      const name = html.slice(i + 2, end).trim();
      for (let k = stack.length - 1; k > 0; k--) {
        if (stack[k].tagName === name) { stack.length = k; break; }
      }
      i = end + 1;
    } else if (html[i] === '<' && /[A-Za-z]/.test(html[i + 1] || '')) {
      const m = /^<([A-Za-z][^\s/>]*)/.exec(html.slice(i));
      const el = new Element(m[1], doc);
      i += m[0].length;
      for (;;) {
        while (/\s/.test(html[i])) i++;
        if (html[i] === '>') { i++; break; }
        if (html[i] === '/' && html[i + 1] === '>') { el.selfClosing = true; i += 2; break; }
        const name = /^[^\s=/>]+/.exec(html.slice(i))[0];
        i += name.length;
        while (/\s/.test(html[i])) i++;
        let value = '';
        if (html[i] === '=') {
          i++;
          while (/\s/.test(html[i])) i++;
          if (html[i] === '"' || html[i] === "'") {
            const q = html[i];
            const end = html.indexOf(q, i + 1);
            value = decode(html.slice(i + 1, end));
            i = end + 1;
          } else {
            const v = /^[^\s>]+/.exec(html.slice(i))[0];
            value = decode(v);
            i += v.length;
          }
        }
        el.attrs.set(name, value);
      }
      add(el);
      const lower = el.tagName.toLowerCase();
      if (el.selfClosing || VOID.has(lower)) continue;
      if (RAW.has(lower)) {
        const end = html.toLowerCase().indexOf(`</${lower}`, i);
        const stop = end < 0 ? html.length : end;
        if (stop > i) el.appendChild(new Text(html.slice(i, stop)));
        i = end < 0 ? html.length : html.indexOf('>', end) + 1;
      } else stack.push(el);
    } else {
      const next = html.indexOf('<', i + 1);
      const stop = next < 0 ? html.length : next;
      add(new Text(decode(html.slice(i, stop))));
      i = stop;
    }
  }
}

// ---------------------------------------------------------------------- test helpers

/** A document with `container` (a div) filled with `html`, attached to the body. */
function createPage(html = '') {
  const doc = new Document();
  const container = doc.createElement('div');
  doc.body.appendChild(container);
  container.innerHTML = html;
  return { doc, container };
}

const fire = (el, type, props) => el.dispatchEvent(makeEvent(type, props));
const press = (el, key, props) => {
  const event = makeEvent('keydown', { key, ...props });
  el.dispatchEvent(event);
  return event;
};

module.exports = { Document, Element, Text, createPage, makeEvent, fire, press, parseSelector, matchesAny };
