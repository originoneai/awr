import { useTranslation } from 'react-i18next';
import { Link, Navigate, useParams } from 'react-router';
import { useCompactViewport } from '../../../lib/viewport';
import { CATALOG_COMPONENTS, useCatalogExamples, type CatalogComponent } from './examples';
import './catalog.css';

function isCatalogComponent(value: string | undefined): value is CatalogComponent {
  return CATALOG_COMPONENTS.some((component) => component === value);
}

/** Development-only, isolated examples. No queries, business mutations or production navigation. */
export function ComponentCatalog() {
  const { t } = useTranslation();
  const { component } = useParams();
  const compactViewport = useCompactViewport();
  const examples = useCatalogExamples();
  if (!isCatalogComponent(component)) return <Navigate to={`/dev/components/${CATALOG_COMPONENTS[0]}`} replace />;
  const example = examples.find((candidate) => candidate.id === component)!;

  return (
    <div className="obs-catalog-page" data-compact={compactViewport || undefined}>
      <header className="obs-catalog-header">
        <h1>{t('display.catalog.title')}</h1>
        <p>{t('display.catalog.subtitle')}</p>
      </header>
      <nav className="obs-catalog-navigation" aria-label={t('display.catalog.navigation')}>
        {CATALOG_COMPONENTS.map((id) => (
          <Link key={id} to={`/dev/components/${id}`} aria-current={id === component ? 'page' : undefined}>
            {t(`display.catalog.components.${id}.name`)}
          </Link>
        ))}
      </nav>
      <main className="obs-catalog-main">
        <div className="obs-catalog-example-heading">
          <div>
            <h2>{t(`display.catalog.components.${component}.name`)}</h2>
            <p>{t(`display.catalog.components.${component}.description`)}</p>
          </div>
          <span>{t('display.catalog.synthetic')}</span>
        </div>
        <div className="obs-catalog-canvas" data-testid="example-canvas" data-component={component}>
          {example.render()}
        </div>
      </main>
    </div>
  );
}
