import { App as AntdApp } from 'antd';
import { createRoot } from 'react-dom/client';
import { MemoryRouter, Route, Routes } from 'react-router';
import '../../../global.css';
import i18n from '../../../i18n';
import { CraftThemeProvider } from '../../CraftThemeProvider';
import { ComponentCatalog } from './ComponentCatalog';

const component = new URLSearchParams(window.location.search).get('component') ?? 'section';
document.title = i18n.t('display.catalog.title');

createRoot(document.getElementById('root')!).render(
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
