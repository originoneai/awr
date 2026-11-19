import { App as AntdApp } from 'antd';
import { readFileSync } from 'node:fs';
import path from 'node:path';
import { render, screen } from '@testing-library/react';
import { MemoryRouter, Route, Routes } from 'react-router';
import { beforeEach, describe, expect, it } from 'vitest';
import i18n from '../../../i18n';
import { CraftThemeProvider } from '../../CraftThemeProvider';
import { CATALOG_COMPONENTS, type CatalogComponent } from './examples';
import { ComponentCatalog } from './ComponentCatalog';

function renderCatalog(component: CatalogComponent) {
  return render(
    <CraftThemeProvider>
      <AntdApp>
        <MemoryRouter initialEntries={[`/dev/components/${component}`]}>
          <Routes>
            <Route path="/dev/components/:component?" element={<ComponentCatalog />} />
          </Routes>
        </MemoryRouter>
      </AntdApp>
    </CraftThemeProvider>,
  );
}

beforeEach(async () => {
  await i18n.changeLanguage('zh-CN');
});

describe('development component catalog', () => {
  it.each(CATALOG_COMPONENTS)('renders the %s example without empty placeholders or raw field names', (component) => {
    renderCatalog(component);
    const canvas = screen.getByTestId('example-canvas');
    expect(canvas.getAttribute('data-component')).toBe(component);
    expect(canvas.textContent?.trim().length).toBeGreaterThan(0);
    expect(screen.getByText('合成数据')).toBeTruthy();
    expect(document.body.textContent).not.toMatch(/\b(?:undefined|null|NaN)\b/);
    expect(document.body.textContent).not.toMatch(/\b(?:work_id|created_at_unix_ms|protocol_version|display_name|next_cursor)\b/);
    expect(document.body.textContent).not.toContain('AWR-');
  });

  it('is a separate development entry, not a production route or production HTML input', () => {
    const router = readFileSync(path.resolve(process.cwd(), 'src/router.tsx'), 'utf8');
    const mainHtml = readFileSync(path.resolve(process.cwd(), 'index.html'), 'utf8');
    expect(router).not.toContain('/dev/components');
    expect(mainHtml).not.toContain('display/catalog');
  });
});
