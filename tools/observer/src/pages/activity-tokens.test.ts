import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';
import { craftCssVariables, craftThemes } from '../theme';

describe('activity named prototype tokens', () => {
  it('pins the column widths and section title and exposes them as CSS', () => {
    expect(craftThemes.light.activity).toEqual({
      columnTime: 168,
      columnActor: 232,
      columnTask: '24%',
      sectionTitleText: 14,
    });
    expect(craftThemes.dark.activity).toEqual(craftThemes.light.activity);
    const variables = craftCssVariables('light');
    expect(variables['--craft-activity-column-time']).toBe('168px');
    expect(variables['--craft-activity-column-actor']).toBe('232px');
    expect(variables['--craft-activity-column-task']).toBe('24%');
    expect(variables['--craft-activity-section-title-text']).toBe('14px');
  });

  it('keeps inline styles, raw colors and one-off absolute dimensions out of activity production source', () => {
    const page = readFileSync('src/pages/ActivityPage.tsx', 'utf8');
    const css = readFileSync('src/pages/ActivityPage.css', 'utf8').replace(/\/\*[\s\S]*?\*\//g, '');
    expect(page).not.toMatch(/\bstyle\s*=/);
    expect(page).not.toMatch(/ACTIVITY_COLUMN_WIDTH/);
    for (const [file, source] of [['ActivityPage.tsx', page], ['ActivityPage.css', css]] as const) {
      expect(source, `${file} has a raw color`).not.toMatch(/#[\da-f]{3,8}\b|\b(?:rgba?|hsla?)\s*\(/i);
      expect(source, `${file} has an untokenized string dimension`).not.toMatch(/['"]\d+(?:px|rem|em|vh|vw)['"]/i);
    }
    const withoutMediaQueries = css.replace(/@media[^\{]+/g, '').replace(/\b1px\b/g, '');
    expect(withoutMediaQueries).not.toMatch(/(?<![\w.-])-?(?:\d+\.\d+|\d+|\.\d+)(?:px|rem|em|vh|vw)\b/i);
    expect(css).not.toContain('24%');
  });

  it('pins the prototype task-link colour and section-title typography', () => {
    const css = readFileSync('src/pages/ActivityPage.css', 'utf8');
    expect(css).toMatch(/\.activity-task-link\s*\{[^}]*color:\s*var\(--craft-color-ink\)/s);
    expect(css).toMatch(/\.activity-task-link:hover,\s*\.activity-task-link:focus-visible\s*\{[^}]*text-decoration:\s*underline/s);
    expect(css).toMatch(/\.activity-section\.ant-card \.ant-card-head-title\s*\{[^}]*font-size:\s*var\(--craft-activity-section-title-text\)[^}]*line-height:\s*var\(--craft-leading-body\)/s);
  });
});
