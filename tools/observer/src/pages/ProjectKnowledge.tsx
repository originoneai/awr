import { useQueries, useQuery } from '@tanstack/react-query';
import { Card, Input, List, Space, Tag, Tooltip, Typography } from 'antd';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { Link } from 'react-router';
import { ObservationEmpty, ObservationForbidden } from '../components/states';
import { isAuthError } from '../lib/api';
import { overviewQueryOptions, workSnapshotQueryOptions } from '../lib/queries';
import { useRevocation } from '../lib/revocation';

interface SourceEntry {
  path: string;
  sha256: string;
  byteLength?: number;
  contentIncluded: boolean;
  text?: string;
  citingWorks: Array<{ key: string; title: string }>;
  goals?: import('../lib/types').WorkContractValue[];
  acceptance?: import('../lib/types').WorkContractValue[];
  /** More than one digest for the same path: the source changed. */
  digestChanged?: boolean;
}

function textOf(value: import('../lib/types').WorkContractValue | undefined): string | null {
  if (value == null) return null;
  if (typeof value === 'string' || typeof value === 'number' || typeof value === 'boolean') return String(value);
  return JSON.stringify(value);
}

function shortSha(sha: string) {
  return sha.length > 12 ? `${sha.slice(0, 12)}…` : sha;
}

/** Assemble the knowledge list from per-work reads, never a project-level read. */
export function ProjectKnowledge({ project }: { project: string }) {
  const { t } = useTranslation();
  const { revoked } = useRevocation();
  const overview = useQuery(overviewQueryOptions(project, revoked));
  const works = overview.data?.works ?? [];
  const snapshots = useQueries({
    queries: works.map((work) => workSnapshotQueryOptions(project, work.key, work.workstream_id, revoked)),
  });
  const [filter, setFilter] = useState('');

  if (revoked || isAuthError(overview.error)) return <ObservationForbidden />;

  const failedReads = snapshots.filter((query) => query.isError).length;
  const entries = new Map<string, SourceEntry>();
  snapshots.forEach((query, index) => {
    const data = query.data?.result.data;
    const work = works[index];
    if (!data || !work) return;
    const sources: Array<{
      path: string;
      sha256: string;
      byte_length?: number;
      text?: string;
      contentIncluded: boolean;
    }> = [
      ...(data.required_specs ?? []).map((spec) => ({
        path: spec.path,
        sha256: spec.sha256,
        byte_length: spec.byte_length,
        text: spec.text,
        contentIncluded: spec.content_included !== false,
      })),
      ...(data.authorized_readable_refs ?? []).map((ref) => ({
        path: ref.path,
        sha256: ref.sha256,
        byte_length: ref.byte_length,
        contentIncluded: false,
      })),
    ];
    for (const source of sources) {
      const key = `${source.path}::${source.sha256}`;
      const existing = entries.get(key);
      const citing = { key: work.key, title: work.title };
      if (existing) {
        if (!existing.citingWorks.some((w) => w.key === citing.key) && !existing.citingWorks.some((w) => w.title === citing.title) && !existing.citingWorks.includes(citing)) {
          existing.citingWorks.push(citing);
        }
        if (source.text && !existing.text) {
          existing.text = source.text;
          existing.contentIncluded = true;
          existing.byteLength = source.byte_length;
        }
        existing.goals = existing.goals ?? data.visible_contract?.goals;
        existing.acceptance = existing.acceptance ?? data.visible_contract?.acceptance;
      } else {
        // A second digest for the same path proves the source changed:
        // never present the older text as current.
        const previous = allEntriesForPath(entries, source.path).find((entry) => entry.sha256 !== source.sha256);
        if (previous) previous.digestChanged = true;
        entries.set(key, {
          path: source.path,
          sha256: source.sha256,
          byteLength: source.byte_length,
          contentIncluded: source.contentIncluded,
          text: source.text,
          citingWorks: [citing],
          goals: data.visible_contract?.goals,
          acceptance: data.visible_contract?.acceptance,
          digestChanged: Boolean(previous),
        });
      }
    }
  });
  const allEntries = Array.from(entries.values());
  function allEntriesForPath(map: Map<string, SourceEntry>, path: string) {
    return Array.from(map.values()).filter((entry) => entry.path === path);
  }
  const visibleEntries = allEntries.filter((entry) => entry.path.toLowerCase().includes(filter.toLowerCase()));
  const [selectedPath, setSelectedPath] = useState<string | null>(null);
  const selected = visibleEntries.find((entry) => entry.path === selectedPath) ?? visibleEntries[0];
  const loadedCount = snapshots.filter((query) => query.data).length;

  return (
    <Space direction="vertical" size={16} style={{ width: '100%' }} data-testid="page-knowledge">
      <Card size="small">
        <Space size={16} wrap>
          <Input.Search
            placeholder={t('knowledge.filterPlaceholder')}
            style={{ maxWidth: 320 }}
            onChange={(event) => setFilter(event.target.value)}
            allowClear
          />
          <Typography.Text type="secondary" data-testid="knowledge-from-works">
            {t('knowledge.fromWorks', { count: loadedCount, total: works.length })}
          </Typography.Text>
          {failedReads > 0 ? (
            <Typography.Text type="warning">{t('knowledge.failedReads', { count: failedReads })}</Typography.Text>
          ) : null}
        </Space>
      </Card>
      {allEntries.length === 0 ? (
        <ObservationEmpty />
      ) : (
        <Space align="start" size={16} style={{ width: '100%' }}>
          <Card size="small" style={{ width: '34%', minWidth: 0 }} title={t('knowledge.sources')}>
            <List
              size="small"
              dataSource={visibleEntries}
              renderItem={(entry) => (
                <List.Item
                  data-testid={`knowledge-source-${entry.path}`}
                  onClick={() => setSelectedPath(entry.path)}
                  style={{ cursor: 'pointer', background: selected?.path === entry.path ? '#EFF6FF' : undefined }}
                >
                  <Space direction="vertical" size={2} style={{ minWidth: 0 }}>
                    <Typography.Text strong>{entry.path}</Typography.Text>
                    <Tooltip title={entry.sha256}>
                      <Typography.Text type="secondary" style={{ fontSize: 12 }}>{shortSha(entry.sha256)}</Typography.Text>
                    </Tooltip>
                  </Space>
                </List.Item>
              )}
            />
          </Card>
          {selected ? (
            <Card size="small" style={{ flex: 1, minWidth: 0 }} title={selected.path}>
              <Space direction="vertical" size={12} style={{ width: '100%' }}>
                <Space size={8} wrap>
                  <Tag color={selected.contentIncluded ? 'success' : 'default'}>
                    {selected.contentIncluded ? t('knowledge.contentIncluded') : t('knowledge.contentMissing')}
                  </Tag>
                  {selected.byteLength != null && selected.text != null && selected.text.length !== selected.byteLength ? (
                    <Tag color="warning">{t('knowledge.partial')}</Tag>
                  ) : null}
                </Space>
                <Typography.Text type="secondary" data-testid="knowledge-citing-works">
                  {t('knowledge.citingWorks')}: {selected.citingWorks.map((work) => work.title).join(', ')}
                </Typography.Text>
                {selected.digestChanged ? (
                  <Typography.Text type="warning">{t('knowledge.sourceChanged')}</Typography.Text>
                ) : selected.text && selected.contentIncluded ? (
                  <Typography.Paragraph style={{ whiteSpace: 'pre-wrap' }}>{selected.text}</Typography.Paragraph>
                ) : (
                  <Typography.Text type="secondary">{t('knowledge.contentMissing')}</Typography.Text>
                )}
                {selected.goals?.length || selected.acceptance?.length ? (
                  <Card size="small" type="inner" title={t('knowledge.contract')}>
                    {selected.goals?.length ? (
                      <List size="small" header={t('workbench.detail.goals')} dataSource={selected.goals} renderItem={(goal) => <List.Item>{textOf(goal)}</List.Item>} />
                    ) : null}
                    {selected.acceptance?.length ? (
                      <List size="small" header={t('workbench.detail.acceptance')} dataSource={selected.acceptance} renderItem={(item) => <List.Item>{textOf(item)}</List.Item>} />
                    ) : null}
                  </Card>
                ) : null}
                <Space size={8} wrap>
                  {selected.citingWorks.map((work) => (
                    <Link key={work.key} to={`/projects/${project}?tab=plan&work=${encodeURIComponent(work.key)}`}>
                      {t('knowledge.openWork', { title: work.title })}
                    </Link>
                  ))}
                </Space>
              </Space>
            </Card>
          ) : null}
        </Space>
      )}
    </Space>
  );
}
