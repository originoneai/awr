import { describe, expect, it } from 'vitest';
import { craftCssVariables, craftThemes, type CraftThemeMode } from './theme';

/**
 * Review round 2 (R7): the values the second component batch had composed from other
 * tokens become named tokens. Names may change, values may not. Pinned for both themes.
 */
describe('named metric, tag and chain tokens', () => {
  it.each<[CraftThemeMode]>([['light'], ['dark']])('pins the shared geometry in %s', (mode) => {
    const tokens = craftThemes[mode];
    expect(tokens.text.metric).toBe(32);
    expect(tokens.text.metricCompact).toBe(28);
    expect(tokens.leading.metric).toBe(40);
    expect(tokens.leading.metricCompact).toBe(36);
    expect(tokens.height.tag).toBe(22);
    expect(tokens.height.chainNode).toBe(104);
    expect(tokens.radius.tag).toBe(6);
    expect(tokens.gap.tag).toBe(6);
    expect(tokens.width.lampRing).toBe(1.5);
    expect(tokens.canvas.grid).toBe(16);
  });

  it('pins the chain canvas colour per theme', () => {
    expect(craftThemes.light.color.canvas).toBe('#EEF1F6');
    expect(craftThemes.dark.color.canvas).toBe('#07080D');
  });

  it.each<[CraftThemeMode]>([['light'], ['dark']])('emits every new token as its CSS variable in %s', (mode) => {
    const variables = craftCssVariables(mode);
    expect(variables['--craft-text-metric']).toBe('32px');
    expect(variables['--craft-text-metric-compact']).toBe('28px');
    expect(variables['--craft-leading-metric']).toBe('40px');
    expect(variables['--craft-leading-metric-compact']).toBe('36px');
    expect(variables['--craft-height-tag']).toBe('22px');
    expect(variables['--craft-height-chain-node']).toBe('104px');
    expect(variables['--craft-radius-tag']).toBe('6px');
    expect(variables['--craft-gap-tag']).toBe('6px');
    expect(variables['--craft-width-lamp-ring']).toBe('1.5px');
    expect(variables['--craft-size-canvas-grid']).toBe('16px');
    expect(variables['--craft-color-canvas']).toBe(craftThemes[mode].color.canvas);
    // The dotted image is built from the theme's own strong hairline colour.
    expect(variables['--craft-canvas-grid-image'])
      .toBe(`radial-gradient(${craftThemes[mode].color.hairlineStrong} 1px, transparent 1px)`);
  });
});
