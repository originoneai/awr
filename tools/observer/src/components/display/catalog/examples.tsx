import { FileText } from 'lucide-react';
import { lazy, Suspense, useState, type ReactNode } from 'react';
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
import { PerspectiveProvider } from '../../../lib/perspective/PerspectiveProvider';
import { PerspectiveSwitcher } from '../../../lib/perspective/PerspectiveSwitcher';
import type { PerspectiveFacts } from '../../../lib/perspective/types';
import type { SearchSource } from '../../../lib/search-index';

// Keep Modal and its styles out of every other isolated component example.
const SearchDialog = lazy(() => import('../../SearchDialog').then((module) => ({ default: module.SearchDialog })));

export const CATALOG_COMPONENTS = [
  'section',
  'pageHeader',
  'pageHeaderCompact',
  'notice',
  'empty',
  'keyValue',
  'rowList',
  'dataTable',
  'detailPanel',
  'timeline',
  'perspective',
  'search',
] as const;

export type CatalogComponent = (typeof CATALOG_COMPONENTS)[number];

const PERSPECTIVE_FACTS = {
  supervisedGroupCount: 1,
  memberGroupProjectCount: 1,
  visibleProjectGroupCount: 3,
  visibleProjectDepartmentCount: 2,
  canViewAllProjects: true,
  directory: { groupsRegistered: true, departmentsRegistered: true },
  pending: { me: 0, lead: 0, dept: 0, top: 0 },
} satisfies PerspectiveFacts;

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
  // The read-only companion table has no openable row, so it renders no arrow column and its
  // last data column carries the outer cell inset (measured by catalog.metrics.spec.ts).
  const referenceRows: DataTableRow[] = [
    {
      key: 'conclusion',
      cells: [t('display.catalog.examples.tableStaticFirst'), t('display.catalog.examples.tableOwnerOne'), t('display.catalog.examples.tableUpdatedOne')],
    },
    {
      key: 'archive',
      cells: [t('display.catalog.examples.tableStaticSecond'), t('display.catalog.examples.tableOwnerTwo'), t('display.catalog.examples.tableUpdatedTwo')],
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
      <Section title={t('display.catalog.examples.tableStaticSection')}>
        <DataTable
          columns={[
            { key: 'work', title: t('display.catalog.examples.tableTask'), width: '52%' },
            { key: 'owner', title: t('display.catalog.examples.tableOwner') },
            { key: 'updated', title: t('display.catalog.examples.tableUpdated'), hideOnSmall: true },
          ]}
          rows={referenceRows}
        />
      </Section>
      {opened ? <Notice>{t('display.catalog.examples.tableOpened', { title: opened })}</Notice> : null}
    </>
  );
}

function SearchExample() {
  const { t, i18n } = useTranslation();
  const [open, setOpen] = useState(true);
  const text = (key: ExampleTextKey) => t(`display.catalog.examples.${key}`);
  const sourceList: SearchSource[] = [
    {
      kind: 'project',
      coverage: 'complete',
      items: [{ key: 'catalog-project', kind: 'project', title: text('project'), meta: text('valueOwner'), path: '/projects/catalog' }],
    },
    {
      kind: 'task',
      coverage: 'complete',
      items: [
        { key: 'catalog-task-one', kind: 'task', title: text('tableFirst'), meta: `${text('project')} · ${text('tableOwnerOne')}`, path: '/projects/catalog?tab=plan' },
        { key: 'catalog-task-two', kind: 'task', title: text('tableSecond'), meta: `${text('project')} · ${text('tableOwnerTwo')}`, path: '/projects/catalog?tab=plan' },
        { key: 'catalog-task-three', kind: 'task', title: text('tableThird'), meta: `${text('project')} · ${text('tableOwnerThree')}`, path: '/projects/catalog?tab=plan' },
      ],
    },
    {
      kind: 'person',
      coverage: 'complete',
      items: [
        { key: 'catalog-person-one', kind: 'person', title: text('tableOwnerOne'), meta: `${text('project')} · ${t('shell.searchProjectMember')}`, path: '/members?scope=catalog' },
        { key: 'catalog-person-two', kind: 'person', title: text('tableOwnerTwo'), meta: `${text('project')} · ${t('shell.searchProjectMember')}`, path: '/members?scope=catalog' },
      ],
    },
    { kind: 'goal', coverage: 'unread', items: [] },
    { kind: 'document', coverage: 'unread', items: [] },
    { kind: 'lesson', coverage: 'unread', items: [] },
  ];
  return (
    <>
      <button type="button" className="obs-catalog-link" onClick={() => setOpen(true)}>{t('shell.search')}</button>
      <Suspense fallback={null}>
        <SearchDialog
          open={open}
          sources={sourceList}
          initialQuery={i18n.language === 'en-US' ? 'Starbridge' : '交'}
          onClose={() => setOpen(false)}
        />
      </Suspense>
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
      id: 'pageHeaderCompact',
      render: () => (
        <PageHeader
          title={text('listTitle')}
          subtitle={text('listSubtitle')}
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
    {
      id: 'perspective',
      render: () => (
        <PerspectiveProvider facts={PERSPECTIVE_FACTS}>
          <PerspectiveSwitcher />
        </PerspectiveProvider>
      ),
    },
    { id: 'search', render: () => <SearchExample /> },
  ];
}
