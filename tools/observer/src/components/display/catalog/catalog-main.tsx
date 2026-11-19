import { App as AntdApp } from 'antd';
import { createRoot } from 'react-dom/client';
import { MemoryRouter, Route, Routes } from 'react-router';
import '../../../global.css';
// Initializes the production default instance: the perspective example is a real shell
// component whose words come from the production vocabulary, not the catalog overlay.
import '../../../i18n';
import { CraftThemeProvider } from '../../CraftThemeProvider';
import { catalogT } from './catalog-i18n';
import { ComponentCatalog } from './ComponentCatalog';

const component = new URLSearchParams(window.location.search).get('component') ?? 'section';
document.title = catalogT('display.catalog.title');

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
