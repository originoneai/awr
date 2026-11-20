import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';
import { craftCssVariables, craftThemes } from '../../theme';

describe('workbench named prototype tokens', () => {
  it('pins every added geometry token and exposes it as CSS', () => {
    expect(craftThemes.light.workbench).toEqual({
      railWidth: 348,
      grid: 20,
      healthHeight: 24,
      healthLamp: 10,
      controlHeight: 32,
      verdictText: 24,
      verdictTextCompact: 20,
      verdictLeading: 32,
      verdictLeadingCompact: 28,
      agentLine: 160,
      agentColumn: 168,
      statusColumn: 108,
      ageColumn: 96,
      waitKindColumn: 136,
      waitDurationColumn: 150,
      behindColumn: 104,
    });
    expect(craftThemes.dark.workbench).toEqual(craftThemes.light.workbench);
    const variables = craftCssVariables('light');
    expect(variables['--craft-workbench-rail-width']).toBe('348px');
    expect(variables['--craft-workbench-grid']).toBe('20px');
    expect(variables['--craft-workbench-health-height']).toBe('24px');
    expect(variables['--craft-workbench-health-lamp']).toBe('10px');
    expect(variables['--craft-workbench-verdict-text']).toBe('24px');
    expect(variables['--craft-workbench-verdict-leading']).toBe('32px');
    expect(variables['--craft-workbench-column-agent']).toBe('168px');
  });

  it('follows the third-draft page theme and removes the instrument shadow in dark mode', () => {
    expect(craftThemes.light.instrument.background).toBe('#FFFFFF');
    expect(craftThemes.light.instrument.shadow).toBe('0 8px 24px rgba(17, 24, 39, 0.10)');
    expect(craftThemes.dark.instrument.background).toBe('#11141C');
    expect(craftThemes.dark.instrument.shadow).toBe('none');
    for (const mode of ['light', 'dark'] as const) {
      const variables = craftCssVariables(mode);
      expect(variables['--craft-instrument-background']).toBe(craftThemes[mode].instrument.background);
      expect(variables['--craft-instrument-grid-image']).toBe('none');
      expect(variables['--craft-instrument-muted']).toBe(craftThemes[mode].instrument.muted);
    }
  });

  it('keeps raw colors and one-off absolute dimensions out of workbench production source', () => {
    const files = ['AgentHandback.tsx', 'WorkbenchFrame.tsx', 'model.ts', 'workbench.css'];
    for (const file of files) {
      const source = readFileSync(`src/components/workbench/${file}`, 'utf8').replace(/\/\*[\s\S]*?\*\//g, '');
      expect(source, `${file} has a raw color`).not.toMatch(/#[\da-f]{3,8}\b|\b(?:rgba?|hsla?)\s*\(/i);
      if (file.endsWith('.css')) {
        const withoutMediaQueries = source.replace(/@media[^\{]+/g, '').replace(/\b1px\b/g, '');
        expect(withoutMediaQueries, `${file} has an untokenized dimension`).not.toMatch(/(?<![\w.-])-?(?:\d+\.\d+|\d+|\.\d+)(?:px|rem|em|vh|vw)\b/i);
      } else {
        expect(source, `${file} has an untokenized dimension`).not.toMatch(/['"]\d+(?:px|rem|em|vh|vw)['"]/i);
      }
    }
  });

  it('keeps the workbench read-only and has no hidden technical disclosure', () => {
    const source = ['AgentHandback.tsx', 'WorkbenchFrame.tsx']
      .map((file) => readFileSync(`src/components/workbench/${file}`, 'utf8')).join('\n');
    expect(source).not.toMatch(/\b(?:fetch|axios|mutate|mutateAsync)\s*\(/);
    expect(source).not.toMatch(/<(?:details|summary)\b/);
    expect(source).not.toMatch(/\bon(?:Dispatch|Claim|Approve|Review|Answer|Merge|Submit|Assign)\s*[=:]/);
  });
});
