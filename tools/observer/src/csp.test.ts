import { describe, expect, it } from 'vitest';
import { installStyleNonceShim } from './csp';

describe('the style nonce shim', () => {
  it('authorizes every style element this app creates with the page nonce', () => {
    window.__CSP_NONCE__ = 'test-page-nonce';
    installStyleNonceShim();
    const style = document.createElement('style');
    expect(style.nonce).toBe('test-page-nonce');
    // The rc measure path (a style injected without a library nonce) is covered.
    document.body.appendChild(style);
    expect(document.querySelector('style')?.nonce).toBe('test-page-nonce');
    style.remove();
    // Other elements are untouched.
    expect(document.createElement('div').nonce).toBe('');
    delete window.__CSP_NONCE__;
  });
});
