import { useQuery } from '@tanstack/react-query';
import { Alert, Button, Flex, Spin, Typography, theme } from 'antd';
import { RefreshCw } from 'lucide-react';
import { useEffect, useRef, type CSSProperties } from 'react';
import { useTranslation } from 'react-i18next';
import page from 'awr-progress-map/project-map/page.js';
import interaction from 'awr-progress-map/project-map/interact.js';
import en from 'awr-progress-map/locales/en.js';
import zh from 'awr-progress-map/locales/zh-CN.js';
import 'awr-progress-map/project-map.css';
import 'awr-progress-map/project-map/interact.css';
import { ObservationEmpty, ObservationForbidden } from '../components/states';
import { apiGet, isAuthError } from '../lib/api';
import { teamProjectSnapshot, type TeamGraphResponse } from '../lib/project-map';
import { useRevocation } from '../lib/revocation';

function MapDrawing({ snapshot, graph }: { snapshot: ReturnType<typeof teamProjectSnapshot>; graph: TeamGraphResponse }) {
  const { t, i18n } = useTranslation();
  const { token } = theme.useToken();
  const host = useRef<HTMLDivElement>(null);
  const restored = useRef<unknown>(undefined);
  const language = i18n.language.startsWith('zh') ? 'zh-CN' : 'en';
  useEffect(() => {
    const container = host.current;
    if (!container) return;
    const messages = page.makeT({ en, 'zh-CN': zh }, language);
    const labels = {
      'map.detail.copy_command': 'projectMaps.copyInstruction',
      'map.detail.technical': 'projectMaps.technical',
      'map.overview.note': 'projectMaps.readOnly',
      'map.project.source': 'projectMaps.source',
      'map.project.revision': 'projectMaps.currentRead',
      'map.overview.note_stalled': 'projectMaps.stalled',
      'map.overview.note_counts': 'projectMaps.dependencies',
      'map.overview.note_gaps': 'projectMaps.moreDetails',
      'map.table.owner': 'projectMaps.owner',
      'map.detail.owner': 'projectMaps.owner',
      'map.detail.claims': 'projectMaps.participating',
      'map.executors.note': 'projectMaps.executors',
      'map.doctor.active_sessions': 'projectMaps.activeAgents',
      'map.executors.title': 'projectMaps.participantsTitle',
      'map.doctor.title': 'projectMaps.activityTitle',
    } as const;
    const translate = (key: string, params?: Record<string, unknown>) => {
      const label = labels[key as keyof typeof labels];
      return label ? t(label, params) : messages(key, params);
    };
    const rendered = page.renderProjectMap(snapshot, undefined, { t: translate, lang: language, interactive: true });
    container.innerHTML = rendered.body;
    // Keep extraction metadata available through the existing technical disclosure.
    const notes = container.querySelector('.notes');
    if (notes) {
      const details = document.createElement('details');
      details.dataset.testid = 'technical-details';
      const summary = document.createElement('summary');
      summary.textContent = t('projectMaps.technical');
      details.append(summary);
      notes.replaceWith(details);
      details.append(notes);
    }
    const mounted = interaction.mount({
      container, snapshot, model: rendered.model, t: translate, restore: restored.current,
      technicalDetails: true,
      commandFor: (key) => {
        const work = graph.items.find((item) => item.external_key === key)!;
        return t('projectMaps.agentInstruction', { query: JSON.stringify({
          protocol_version: 1, op: 'work.prepare', work_id: work.work_id, workstream_id: work.workstream_id,
        }) });
      },
    });
    return () => {
      restored.current = mounted.state;
      mounted.destroy();
      container.replaceChildren();
    };
  }, [snapshot, graph, language, t]);
  const variables = {
    '--ink': token.colorText, '--ink-2': token.colorTextSecondary, '--ink-3': token.colorTextTertiary,
    '--surface': token.colorBgContainer, '--surface-2': token.colorFillQuaternary,
    '--line': token.colorBorderSecondary, '--line-soft': token.colorBorderSecondary,
    '--accent': token.colorPrimary, '--btn-ink': token.colorTextLightSolid,
    '--warn': token.colorWarning, '--crit': token.colorError, '--shadow-1': 'none', '--shadow-2': token.boxShadowSecondary,
  } as CSSProperties;
  return <div id="view-map" style={variables}><div ref={host} className="pm-scope" data-testid="project-map-drawing" /></div>;
}

/** The Team and personal surfaces share rendering, layout and interactions. */
export function ProjectMaps({ project }: { project: string }) {
  const { t } = useTranslation();
  const { revoked } = useRevocation();
  const graph = useQuery({
    queryKey: ['team', 'graph', project],
    queryFn: async () => {
      const response = await apiGet<TeamGraphResponse>(`/api/team/graph?project=${encodeURIComponent(project)}`);
      return { response, snapshot: teamProjectSnapshot(response) };
    },
    enabled: Boolean(project) && !revoked,
    retry: false,
    refetchOnWindowFocus: false,
    refetchOnReconnect: false,
    refetchInterval: false,
    staleTime: 0,
  });
  if (revoked || isAuthError(graph.error)) return <ObservationForbidden />;
  if (graph.isPending) return <Spin aria-label={t('common.loading')} />;
  const hiddenDependencies = graph.data?.response.items.some((item) => !item.dependencies_complete);
  return (
    <Flex vertical gap={16} data-testid="project-maps">
      <Flex wrap gap={12} align="center" justify="space-between">
        <Typography.Title level={5} style={{ margin: 0 }}>{t('projectMaps.title')}</Typography.Title>
        <Button icon={<RefreshCw size={16} strokeWidth={1.5} />} loading={graph.isFetching} onClick={() => void graph.refetch()}>
          {t('projectMaps.refresh')}
        </Button>
      </Flex>
      {graph.error ? <Alert type="warning" showIcon title={t('projectMaps.readFailed')} /> : null}
      {hiddenDependencies ? <Alert type="info" showIcon title={t('projectMaps.restricted')} /> : null}
      {graph.data?.response.demo ? <Typography.Text type="secondary">{t('projectMaps.demo')}</Typography.Text> : null}
      {graph.data?.response.items.length ? <MapDrawing key={project} snapshot={graph.data.snapshot} graph={graph.data.response} /> : graph.data ? <ObservationEmpty /> : null}
    </Flex>
  );
}
