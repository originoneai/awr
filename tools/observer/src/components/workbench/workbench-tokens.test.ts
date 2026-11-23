import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';
import { craftCssVariables, craftThemes } from '../../theme';

describe('workbench named prototype tokens', () => {
  it('pins every added geometry token and exposes it as CSS', () => {
    expect(craftThemes.light.workbench).toEqual({
      railWidth: 348,
      healthHeight: 24,
      healthLamp: 10,
      healthPaddingInline: 10,
      captionText: 12,
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
      waitBarMinWidth: 48,
      behindColumn: 104,
      chainNodeMinWidth: 132,
    });
    expect(craftThemes.dark.workbench).toEqual(craftThemes.light.workbench);
    const variables = craftCssVariables('light');
    expect(variables['--craft-workbench-rail-width']).toBe('348px');
    expect(variables['--craft-workbench-health-height']).toBe('24px');
    expect(variables['--craft-workbench-health-lamp']).toBe('10px');
    expect(variables['--craft-workbench-health-padding-inline']).toBe('10px');
    expect(variables['--craft-workbench-caption-text']).toBe('12px');
    expect(variables['--craft-workbench-verdict-text']).toBe('24px');
    expect(variables['--craft-workbench-verdict-leading']).toBe('32px');
    expect(variables['--craft-workbench-wait-bar-min']).toBe('48px');
    expect(variables['--craft-workbench-column-agent']).toBe('168px');
    expect(variables['--craft-workbench-chain-node-min']).toBe('132px');
  });

  it('has no second instrument palette and keeps the prototype dark floating shadow', () => {
    expect(craftThemes.light).not.toHaveProperty('instrument');
    expect(craftThemes.dark).not.toHaveProperty('instrument');
    const source = readFileSync('src/theme.ts', 'utf8');
    expect(source).not.toMatch(/InstrumentPalette|--craft-instrument-|instrument\s*:/);
    for (const mode of ['light', 'dark'] as const) {
      const variables = craftCssVariables(mode);
      expect(Object.keys(variables).filter((key) => key.startsWith('--craft-instrument-'))).toEqual([]);
      expect(variables['--craft-workbench-grid']).toBeUndefined();
      expect(variables['--craft-shadow-floating']).toBe(craftThemes[mode].shadow.floating);
    }
    expect(craftThemes.dark.shadow.floating).toBe('0 16px 40px rgba(0, 0, 0, 0.50)');
  });

  it('keeps raw colors and one-off absolute dimensions out of workbench production source', () => {
    const files = ['AgentHandback.tsx', 'WorkbenchFrame.tsx', 'DepartmentWorkbench.tsx', 'model.ts', 'department-model.ts', 'workbench.css'];
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
    const source = ['AgentHandback.tsx', 'WorkbenchFrame.tsx', 'DepartmentWorkbench.tsx']
      .map((file) => readFileSync(`src/components/workbench/${file}`, 'utf8')).join('\n');
    expect(source).not.toMatch(/\b(?:fetch|axios|mutate|mutateAsync)\s*\(/);
    expect(source).not.toMatch(/<(?:details|summary)\b/);
    expect(source).not.toMatch(/\bon(?:Dispatch|Claim|Approve|Review|Answer|Merge|Submit|Assign)\s*[=:]/);
  });
});
