import { FileText } from 'lucide-react';
import { useState, type ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import type zhCN from '../../../i18n/zh-CN';
import { DataTable, type DataTableRow } from '../DataTable';
import { DetailPanel } from '../DetailPanel';
import { DisplayIcon } from '../DisplayIcon';
import { EmptyState } from '../EmptyState';
import { KeyValueTable } from '../KeyValueTable';
import { Notice } from '../Notice';
import { PageHeader } from '../PageHeader';
import { RowList } from '../RowList';
import { Section } from '../Section';
import { Timeline } from '../Timeline';

export const CATALOG_COMPONENTS = [
  'section',
  'pageHeader',
  'notice',
  'empty',
  'keyValue',
  'rowList',
  'dataTable',
  'detailPanel',
  'timeline',
] as const;

export type CatalogComponent = (typeof CATALOG_COMPONENTS)[number];

export interface CatalogExample {
  id: CatalogComponent;
  render: () => ReactNode;
}

function RowListExample() {
  const { t } = useTranslation();
  const [opened, setOpened] = useState<string | null>(null);
  const titles = [t('display.catalog.examples.rowFirst'), t('display.catalog.examples.rowSecond')];
  return (
    <>
      <Section title={t('display.catalog.examples.rowSection')} count={3}>
        <RowList
          items={[
            {
              key: 'agreement',
              title: titles[0],
              meta: t('display.catalog.examples.rowFirstMeta'),
              leading: <DisplayIcon icon={FileText} />,
              trailing: t('display.catalog.examples.rowTrailing'),
              selected: true,
              onOpen: () => setOpened(titles[0]),
            },
            {
              key: 'checks',
              title: titles[1],
              meta: t('display.catalog.examples.rowSecondMeta'),
              leading: <DisplayIcon icon={FileText} />,
              onOpen: () => setOpened(titles[1]),
            },
            {
              key: 'notes',
              title: t('display.catalog.examples.rowThird'),
              meta: t('display.catalog.examples.rowThirdMeta'),
              leading: <DisplayIcon icon={FileText} />,
            },
          ]}
        />
      </Section>
      {opened ? <Notice>{t('display.catalog.examples.rowOpened', { title: opened })}</Notice> : null}
    </>
  );
}

function DataTableExample() {
  const { t } = useTranslation();
  const [opened, setOpened] = useState<string | null>(null);
  const titles = [
    t('display.catalog.examples.tableFirst'),
    t('display.catalog.examples.tableSecond'),
    t('display.catalog.examples.tableThird'),
  ];
  const rows: DataTableRow[] = [
    {
      key: 'interaction',
      cells: [titles[0], t('display.catalog.examples.tableOwnerOne'), t('display.catalog.examples.tableUpdatedOne')],
      onOpen: () => setOpened(titles[0]),
    },
    {
      key: 'evidence',
      cells: [titles[1], t('display.catalog.examples.tableOwnerTwo'), t('display.catalog.examples.tableUpdatedTwo')],
      highlighted: true,
      onOpen: () => setOpened(titles[1]),
    },
    {
      key: 'notes',
      cells: [titles[2], t('display.catalog.examples.tableOwnerThree'), t('display.catalog.examples.tableUpdatedThree')],
    },
  ];
  return (
    <>
      <Section
        title={t('display.catalog.examples.tableSection')}
        count={rows.length}
        extra={<span className="obs-section-extra">{t('display.catalog.examples.tableExtra')}</span>}
      >
        <DataTable
          columns={[
            { key: 'work', title: t('display.catalog.examples.tableTask'), width: '52%' },
            { key: 'owner', title: t('display.catalog.examples.tableOwner') },
            { key: 'updated', title: t('display.catalog.examples.tableUpdated'), hideOnSmall: true },
          ]}
          rows={rows}
        />
      </Section>
      {opened ? <Notice>{t('display.catalog.examples.tableOpened', { title: opened })}</Notice> : null}
    </>
  );
}

type ExampleTextKey = keyof typeof zhCN.display.catalog.examples;

export function useCatalogExamples(): CatalogExample[] {
  const { t } = useTranslation();
  const text = (key: ExampleTextKey) => t(`display.catalog.examples.${key}`);
  return [
    {
      id: 'section',
      render: () => (
        <Section
          title={text('sectionTitle')}
          count={3}
          extra={<span className="obs-section-extra">{text('sectionExtra')}</span>}
          padded
        >
          <p>{text('sectionBody')}</p>
          <p className="obs-catalog-dim">{text('sectionSecondary')}</p>
        </Section>
      ),
    },
    {
      id: 'pageHeader',
      render: () => (
        <PageHeader
          variant="detail"
          title={text('project')}
          summary={text('headerSummary')}
          meta={(
            <>
              <span>{text('headerOwner')}</span>
              <span>{text('headerNext')}</span>
            </>
          )}
          actions={<a className="obs-catalog-link" href="?component=dataTable">{text('headerAction')}</a>}
          onBack={() => window.history.back()}
        />
      ),
    },
    {
      id: 'notice',
      render: () => (
        <div className="obs-catalog-stack">
          <Notice title={text('noticeTitle')}>{text('noticeBody')}</Notice>
          <Notice tone="warning" title={text('noticeWarningTitle')}>{text('noticeWarningBody')}</Notice>
        </div>
      ),
    },
    {
      id: 'empty',
      render: () => (
        <Section title={text('project')}>
          <EmptyState title={text('emptyTitle')} description={text('emptyDescription')} />
        </Section>
      ),
    },
    {
      id: 'keyValue',
      render: () => (
        <Section title={text('project')}>
          <KeyValueTable
            items={[
              { key: 'owner', label: text('keyOwner'), value: text('valueOwner') },
              { key: 'stage', label: text('keyStage'), value: text('valueStage') },
              { key: 'evidence', label: text('keyEvidence'), value: text('valueEvidence') },
              { key: 'next', label: text('keyNext'), value: text('valueNext') },
            ]}
          />
        </Section>
      ),
    },
    { id: 'rowList', render: () => <RowListExample /> },
    { id: 'dataTable', render: () => <DataTableExample /> },
    {
      id: 'detailPanel',
      render: () => (
        <DetailPanel
          extra={<span className="obs-section-extra">{text('detailExtra')}</span>}
          tabs={[
            {
              key: 'progress',
              label: text('detailProgress'),
              count: 3,
              content: <div className="obs-catalog-detail-copy">{text('detailProgressBody')}</div>,
              footer: text('detailFooter'),
            },
            {
              key: 'evidence',
              label: text('detailEvidence'),
              count: 3,
              content: <div className="obs-catalog-detail-copy">{text('detailEvidenceBody')}</div>,
              footer: text('detailFooter'),
            },
            {
              key: 'waiting',
              label: text('detailWaiting'),
              count: 1,
              content: <div className="obs-catalog-detail-copy">{text('detailWaitingBody')}</div>,
              footer: text('detailFooter'),
            },
          ]}
        />
      ),
    },
    {
      id: 'timeline',
      render: () => (
        <Section title={text('timelineSection')} count={3}>
          <Timeline
            items={[
              {
                key: 'integration',
                time: text('timelineOneTime'),
                title: text('timelineOneTitle'),
                detail: text('timelineOneDetail'),
                tone: 'positive',
              },
              {
                key: 'evidence',
                time: text('timelineTwoTime'),
                title: text('timelineTwoTitle'),
                detail: text('timelineTwoDetail'),
              },
              {
                key: 'review',
                time: text('timelineThreeTime'),
                title: text('timelineThreeTitle'),
                detail: text('timelineThreeDetail'),
                tone: 'current',
              },
            ]}
          />
        </Section>
      ),
    },
  ];
}
