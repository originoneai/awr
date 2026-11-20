import { FileText } from 'lucide-react';
import { lazy, Suspense, useState, type ReactNode } from 'react';
import {
  HealthTag,
  TaskStatusTag,
  AcceptanceList,
  Chain,
  CompletionBar,
  CompletionLegend,
  DataTable,
  DetailPanel,
  DisplayIcon,
  EmptyState,
  KeyValueTable,
  KpiStrip,
  Notice,
  PageHeader,
  RowList,
  Section,
  Timeline,
  TodoRail,
} from '..';
import type { DataTableRow } from '..';
import type { ChainConnector, ChainNode, DerivedTaskStatus } from '../../../core/overview/types';
import { TASK_STATUSES } from '../../../core/overview/types';
import { PerspectiveProvider } from '../../../lib/perspective/PerspectiveProvider';
import { PerspectiveSwitcher } from '../../../lib/perspective/PerspectiveSwitcher';
import type { PerspectiveFacts } from '../../../lib/perspective/types';
import type { SearchSource } from '../../../lib/search-index';
import { catalogI18n, useCatalogTranslation } from './catalog-i18n';
import type { CatalogExampleTextKey } from './catalog-messages';

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
  'kpiStrip',
  'statusTag',
  'completionBar',
  'chain',
  'todoRail',
  'acceptanceList',
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
  const { t } = useCatalogTranslation();
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
  const { t } = useCatalogTranslation();
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

function KpiStripExample() {
  const { t } = useCatalogTranslation();
  const [opened, setOpened] = useState<string | null>(null);
  const text = (key: CatalogExampleTextKey) => t(`display.catalog.examples.${key}`);
  const labels = [text('kpiOne'), text('kpiTwo'), text('kpiThree'), text('kpiFour')];
  return (
    <>
      <KpiStrip
        items={[
          {
            key: 'verified',
            label: labels[0],
            value: '5',
            unit: text('kpiUnitItems'),
            change: { text: text('kpiOneChange'), tone: 'good', trend: 'up' },
            onOpen: () => setOpened(labels[0]),
          },
          {
            key: 'running',
            label: labels[1],
            value: '3',
            unit: text('kpiUnitItems'),
            change: { text: text('kpiTwoChange'), tone: 'warn' },
            onOpen: () => setOpened(labels[1]),
          },
          {
            key: 'waiting',
            label: labels[2],
            value: '2',
            unit: text('kpiUnitAgents'),
            change: { text: text('kpiThreeChange'), tone: 'bad' },
            onOpen: () => setOpened(labels[2]),
          },
          {
            key: 'mine',
            label: labels[3],
            value: '2',
            unit: text('kpiUnitItems'),
            change: { text: text('kpiFourChange') },
            emphasized: true,
            onOpen: () => setOpened(labels[3]),
          },
        ]}
      />
      {opened ? <Notice>{t('display.catalog.examples.kpiOpened', { label: opened })}</Notice> : null}
    </>
  );
}

const TASK_STATUS_SAMPLE: readonly DerivedTaskStatus[] = [...TASK_STATUSES, 'unknown'];

function StatusTagExample() {
  const { t } = useCatalogTranslation();
  return (
    <Section title={t('display.catalog.examples.project')} padded>
      <div className="obs-catalog-tag-row">
        <span className="obs-catalog-dim">{t('display.catalog.examples.statusProjectGroup')}</span>
        {(['ok', 'risk', 'off', 'none', 'info'] as const).map((status) => (
          <HealthTag key={status} status={status} />
        ))}
      </div>
      <div className="obs-catalog-tag-row">
        <span className="obs-catalog-dim">{t('display.catalog.examples.statusTaskGroup')}</span>
        {TASK_STATUS_SAMPLE.map((status) => (
          <TaskStatusTag key={status} status={status} live={status === 'running'} />
        ))}
      </div>
    </Section>
  );
}

function CompletionBarExample() {
  const { t } = useCatalogTranslation();
  const text = (key: CatalogExampleTextKey) => t(`display.catalog.examples.${key}`);
  return (
    <Section
      title={text('completionSection')}
      padded
      extra={<CompletionLegend />}
    >
      <div className="obs-catalog-completion-row">
        <span>{text('completionRowOne')}</span>
        <CompletionBar counts={{ verified: 6, reported: 1, inProgress: 2, notStarted: 1 }} />
      </div>
      <div className="obs-catalog-completion-row">
        <span>{text('completionRowTwo')}</span>
        <CompletionBar counts={{ verified: 4, reported: 0, inProgress: 1, notStarted: 0 }} />
      </div>
    </Section>
  );
}

function ChainExample() {
  const { t } = useCatalogTranslation();
  const [opened, setOpened] = useState<string | null>(null);
  const text = (key: CatalogExampleTextKey) => t(`display.catalog.examples.${key}`);
  const nodes: readonly ChainNode[] = [
    { id: 'demo:root', kind: 'root', root: true, tone: 'negative', task: null, commitment: null },
    { id: 'demo:task:interface', kind: 'task', root: false, tone: null, task: null, commitment: null },
    { id: 'demo:task:integration', kind: 'task', root: false, tone: null, task: null, commitment: null },
    { id: 'demo:commitment:delivery', kind: 'commitment', root: false, tone: null, task: null, commitment: { kind: 'project' } },
  ];
  const connectors: readonly ChainConnector[] = [
    { fromNodeId: 'demo:root', toNodeId: 'demo:task:interface', kind: 'blocks' },
    { fromNodeId: 'demo:task:interface', toNodeId: 'demo:task:integration', kind: 'depends' },
    { fromNodeId: 'demo:task:integration', toNodeId: 'demo:commitment:delivery', kind: 'affects' },
  ];
  const present = (node: ChainNode) => {
    switch (node.id) {
      case 'demo:root':
        return {
          kicker: t('display.chain.rootKicker'),
          title: text('chainRootTitle'),
          ownerName: t('display.catalog.examples.tableOwnerTwo'),
          queue: [
            { title: text('chainQueueOne'), days: 2 },
            { title: text('chainQueueTwo'), days: 1 },
          ],
          queueNote: { queued: 2, oldestDays: 2 },
        };
      case 'demo:task:interface':
        return {
          kicker: t('display.chain.taskKicker'),
          title: text('chainTaskOneTitle'),
          meta: text('chainTaskOneMeta'),
          status: 'waiting' as const,
        };
      case 'demo:task:integration':
        return {
          kicker: t('display.chain.taskKicker'),
          title: text('chainTaskTwoTitle'),
          meta: text('chainTaskTwoMeta'),
          status: 'running' as const,
          you: true,
          // The example shows the hot outline here so the state has a baseline; on a real
          // page the page draws it on the node whose rail card is hovered or focused.
          highlighted: true,
        };
      default:
        return {
          kicker: text('chainEndKicker'),
          title: text('chainEndTitle'),
          meta: text('chainEndMeta'),
          health: 'risk' as const,
          progress: { done: 4, total: 6 },
        };
    }
  };
  return (
    // No Section frame: the chain canvas is its own container, as .why is inside .inst (R9).
    <>
      <Chain
        nodes={nodes}
        connectors={connectors}
        present={present}
        onOpen={(node) => setOpened(present(node).title)}
        footer={<span>{text('chainFooter')}</span>}
      />
      {opened ? <Notice>{t('display.catalog.examples.chainOpened', { title: opened })}</Notice> : null}
    </>
  );
}

function TodoRailExample() {
  const { t } = useCatalogTranslation();
  const [opened, setOpened] = useState<string | null>(null);
  const [openedAll, setOpenedAll] = useState(false);
  const text = (key: CatalogExampleTextKey) => t(`display.catalog.examples.${key}`);
  const items = [
    {
      key: 'empty-state',
      kind: text('todoKindAnswer'),
      title: text('todoFirstTitle'),
      from: text('todoFirstFrom'),
      age: text('todoFirstAge'),
      detail: text('todoFirstDetail'),
      onOpen: () => setOpened(text('todoFirstTitle')),
    },
    {
      key: 'second-reviewer',
      kind: text('todoKindDecision'),
      title: text('todoSecondTitle'),
      age: text('todoSecondAge'),
      onOpen: () => setOpened(text('todoSecondTitle')),
    },
    {
      key: 'stage-checks',
      kind: text('todoKindReview'),
      title: text('todoThirdTitle'),
      age: text('todoThirdAge'),
      onOpen: () => setOpened(text('todoThirdTitle')),
    },
    {
      key: 'delivery-notes',
      kind: text('todoKindDecision'),
      title: text('todoFourthTitle'),
      age: text('todoFourthAge'),
      onOpen: () => setOpened(text('todoFourthTitle')),
    },
    {
      key: 'demo-archive',
      kind: text('todoKindAnswer'),
      title: text('todoFifthTitle'),
      age: text('todoFifthAge'),
      onOpen: () => setOpened(text('todoFifthTitle')),
    },
  ];
  return (
    <>
      {/* The rail is a narrow column, the way the prototype composes it inside the panel. */}
      <div className="obs-catalog-rail-column">
        <TodoRail
          title={text('todoTitle')}
          items={items}
          onViewAll={() => setOpenedAll(true)}
        />
      </div>
      {opened ? <Notice>{t('display.catalog.examples.todoViewed', { title: opened })}</Notice> : null}
      {openedAll ? <Notice>{t('display.catalog.examples.todoViewedAll')}</Notice> : null}
    </>
  );
}

function AcceptanceListExample() {
  const { t } = useCatalogTranslation();
  const text = (key: CatalogExampleTextKey) => t(`display.catalog.examples.${key}`);
  return (
    // Unpadded: the prototype's acceptance rows carry the block's own 20px inset (R9).
    <Section title={text('acceptanceSection')} count={4}>
      <AcceptanceList
        items={[
          { key: 'lookup', title: text('acceptanceOne'), evidence: text('acceptanceOneEvidence'), status: 'supported' },
          { key: 'keep-input', title: text('acceptanceTwo'), evidence: text('acceptanceTwoEvidence'), status: 'waiting' },
          { key: 'repeat-submit', title: text('acceptanceThree'), evidence: text('acceptanceThreeEvidence'), status: 'failed' },
          { key: 'scenarios', title: text('acceptanceFour'), status: 'notStarted' },
        ]}
      />
      <p className="obs-catalog-pnote">{text('acceptanceNote')}</p>
    </Section>
  );
}

function SearchExample() {
  const { t } = useCatalogTranslation();
  const [open, setOpen] = useState(true);
  const text = (key: CatalogExampleTextKey) => t(`display.catalog.examples.${key}`);
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
          initialQuery={catalogI18n.language === 'en-US' ? 'Starbridge' : '交'}
          onClose={() => setOpen(false)}
        />
      </Suspense>
    </>
  );
}

export function useCatalogExamples(): CatalogExample[] {
  const { t } = useCatalogTranslation();
  const text = (key: CatalogExampleTextKey) => t(`display.catalog.examples.${key}`);
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
    { id: 'kpiStrip', render: () => <KpiStripExample /> },
    { id: 'statusTag', render: () => <StatusTagExample /> },
    { id: 'completionBar', render: () => <CompletionBarExample /> },
    { id: 'chain', render: () => <ChainExample /> },
    { id: 'todoRail', render: () => <TodoRailExample /> },
    { id: 'acceptanceList', render: () => <AcceptanceListExample /> },
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
