import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';
import { craftThemes } from './theme';

const html = readFileSync('index.html', 'utf8');
const css = readFileSync('src/theme-fallback.css', 'utf8');

function declarations(pattern: RegExp): Record<string, string> {
  const block = css.match(pattern)?.[1];
  expect(block, `Missing CSS block ${pattern}`).toBeTruthy();
  return Object.fromEntries([...(block ?? '').matchAll(/(--[\w-]+)\s*:\s*([^;]+)/g)]
    .map((match) => [match[1], match[2].trim()]));
}

describe('pre-bundle theme and first paint', () => {
  it('runs a small nonce-authorized saved/system theme bootstrap before the application bundle', () => {
    const scripts = [...html.matchAll(/<script nonce="__CSP_NONCE_ATTR__">([\s\S]*?)<\/script>/g)]
      .map((match) => match[1]);
    const bootstrap = scripts.find((script) => script.includes('awr-observer-theme'));
    expect(bootstrap).toBeTruthy();
    expect(bootstrap?.length).toBeLessThan(900);
    expect(bootstrap).toContain('localStorage.getItem("awr-observer-theme")');
    expect(bootstrap).toContain('matchMedia("(prefers-color-scheme: dark)")');
    expect(bootstrap).toContain('document.documentElement.dataset.theme = theme');
    expect(bootstrap).toContain('document.documentElement.style.colorScheme = theme');
    expect(bootstrap).toMatch(/try\s*\{[^}]+\}\s*catch/);
    expect(html.indexOf(bootstrap ?? '')).toBeLessThan(html.indexOf('/src/main.tsx'));
  });

  it('pins the light, explicit-dark and system-dark static fallbacks to the theme tokens', () => {
    const light = declarations(/:root\s*\{([^}]*)\}/);
    const explicitDark = declarations(/:root\[data-theme="dark"\]\s*\{([^}]*)\}/);
    const systemDark = declarations(/@media \(prefers-color-scheme: dark\)\s*\{\s*:root:not\(\[data-theme="light"\]\)\s*\{([^}]*)\}/);

    expect(light['--craft-color-page']).toBe(craftThemes.light.color.page);
    expect(light['--craft-color-ink']).toBe(craftThemes.light.color.ink);
    for (const fallback of [explicitDark, systemDark]) {
      expect(fallback['--craft-color-page']).toBe(craftThemes.dark.color.page);
      expect(fallback['--craft-color-ink']).toBe(craftThemes.dark.color.ink);
    }
  });

  it('keeps system dark available when the bootstrap cannot set a data attribute', () => {
    expect(css).toContain('@media (prefers-color-scheme: dark)');
    expect(css).toContain(':root:not([data-theme="light"])');
    expect(css).toContain('color-scheme: dark');
  });
});
