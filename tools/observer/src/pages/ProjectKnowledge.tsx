import { useQuery } from '@tanstack/react-query';
import { Card, Col, Input, List, Pagination, Row, Space, Tag, Tooltip, Typography, theme } from 'antd';
import { useState } from 'react';
import { Stamp } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { Link, useSearchParams } from 'react-router';
import { ObservationEmpty, ObservationForbidden } from '../components/states';
import { rawObservationText, utf8ByteLength } from '../core/observation';
import { isAuthError } from '../lib/api';
import { OBSERVATION_PAGE_SIZE, validObservationPage } from '../lib/pagination';
import { overviewQueryOptions } from '../lib/queries';
import { useRevocation } from '../lib/revocation';
import { useTextDigest, type DigestCheck } from '../lib/text-digest';
import { useWorkSnapshots } from '../lib/work-snapshots';
import { craftSelectionStyle } from '../theme';
import type { WorkContractValue } from '../lib/types';

interface CitingWork {
  key: string;
  title: string;
  goals?: WorkContractValue[];
  acceptance?: WorkContractValue[];
}

interface SourceEntry {
  path: string;
  sha256: string;
  byteLength?: number;
  contentIncluded: boolean;
  text?: string;
  citingWorks: CitingWork[];
}

function shortSha(sha: string) {
  return sha.length > 12 ? `${sha.slice(0, 12)}…` : sha;
}

/** The words of the digest check; a check that has not finished reads as not verified. */
const DIGEST_LABEL = {
  verified: 'knowledge.digestVerified',
  mismatch: 'knowledge.digestMismatch',
  unverified: 'knowledge.digestUnverified',
  pending: 'knowledge.digestUnverified',
} as const satisfies Record<DigestCheck, string>;

/** Assemble the knowledge list from per-work reads, never a project-level read. */
export function ProjectKnowledge({ project }: { project: string }) {
  const { t } = useTranslation();
  const { token } = theme.useToken();
  const { revoked } = useRevocation();
  const overview = useQuery(overviewQueryOptions(project, revoked));
  const works = overview.data?.works ?? [];
  const [searchParams, setSearchParams] = useSearchParams();
  const page = Math.min(
    validObservationPage(searchParams.get('page')),
    Math.max(1, Math.ceil(works.length / OBSERVATION_PAGE_SIZE)),
  );
  const visibleWorks = works.slice((page - 1) * OBSERVATION_PAGE_SIZE, page * OBSERVATION_PAGE_SIZE);
  const snapshots = useWorkSnapshots(project, visibleWorks, revoked);
  const [filter, setFilter] = useState('');
  const [selectedKey, setSelectedKey] = useState<string | null>(null);

  const failedReads = snapshots.filter((query) => query.isError).length;
  const entries = new Map<string, SourceEntry>();
  snapshots.forEach((query, index) => {
    const data = query.data?.result.data;
    const work = visibleWorks[index];
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
      const citing: CitingWork = {
        key: work.key,
        title: work.title,
        goals: data.visible_contract?.goals,
        acceptance: data.visible_contract?.acceptance,
      };
      if (existing) {
        // De-duplicate citing works by key only (same titles are allowed).
        if (!existing.citingWorks.some((w) => w.key === citing.key)) {
          existing.citingWorks.push(citing);
        }
        if (source.text && !existing.text) {
          existing.text = source.text;
          existing.contentIncluded = true;
          existing.byteLength = source.byte_length;
        }
      } else {
        entries.set(key, {
          path: source.path,
          sha256: source.sha256,
          byteLength: source.byte_length,
          contentIncluded: source.contentIncluded,
          text: source.text,
          citingWorks: [citing],
        });
      }
    }
  });
  const allEntries = Array.from(entries.values());
  function allEntriesForPath(map: Map<string, SourceEntry>, path: string) {
    return Array.from(map.values()).filter((entry) => entry.path === path);
  }
  const visibleEntries = allEntries.filter((entry) => entry.path.toLowerCase().includes(filter.toLowerCase()));
  const selected = visibleEntries.find((entry) => `${entry.path}::${entry.sha256}` === selectedKey) ?? visibleEntries[0];
  const loadedCount = snapshots.filter((query) => query.data).length;
  const selectedDigests = selected ? allEntriesForPath(entries, selected.path).length : 0;

  // The displayed text is verified against its declared digest; without Web Crypto the badge reads
  // not verified, never verified.
  const digestCheck = useTextDigest(selected?.contentIncluded ? selected.text : null, selected?.sha256);

  if (revoked || isAuthError(overview.error)) return <ObservationForbidden />;

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
      <Pagination
        current={page}
        total={works.length}
        pageSize={OBSERVATION_PAGE_SIZE}
        showSizeChanger={false}
        onChange={(nextPage) => {
          const next = new URLSearchParams(searchParams);
          if (nextPage > 1) next.set('page', String(nextPage));
          else next.delete('page');
          setSearchParams(next);
        }}
        style={{ display: 'flex', justifyContent: 'flex-end' }}
      />
      {allEntries.length === 0 ? (
        <ObservationEmpty />
      ) : (
        <Row gutter={[16, 16]} style={{ width: '100%' }}>
          <Col xs={24} lg={8}>
            <Card size="small" style={{ width: '100%' }} title={t('knowledge.sources')}>
              <List
                size="small"
                dataSource={visibleEntries}
                rowKey={(entry) => `${entry.path}::${entry.sha256}`}
                renderItem={(entry) => (
                  <List.Item
                    data-testid={`knowledge-source-${entry.path}-${entry.sha256.slice(0, 12)}`}
                    onClick={() => setSelectedKey(`${entry.path}::${entry.sha256}`)}
                    style={craftSelectionStyle(token, Boolean(selected && `${selected.path}::${selected.sha256}` === `${entry.path}::${entry.sha256}`))}
                  >
                    <Space direction="vertical" size={2} style={{ minWidth: 0 }}>
                      <Typography.Text strong style={{ overflowWrap: 'anywhere' }}>{entry.path}</Typography.Text>
                      <Tooltip title={entry.sha256}>
                        <Typography.Text type="secondary" style={{ fontSize: 12 }}>{shortSha(entry.sha256)}</Typography.Text>
                      </Tooltip>
                    </Space>
                  </List.Item>
                )}
              />
            </Card>
          </Col>
          {selected ? (
            <Col xs={24} lg={16}>
              <Card size="small" style={{ width: '100%', minWidth: 0 }} title={<Typography.Text style={{ overflowWrap: 'anywhere' }}>{selected.path}</Typography.Text>}>
                <Space direction="vertical" size={12} style={{ width: '100%' }}>
                  <Space size={8} wrap>
                    <Tag color={selected.contentIncluded ? 'success' : 'default'}>
                      {selected.contentIncluded ? t('knowledge.contentIncluded') : t('knowledge.contentMissing')}
                    </Tag>
                    {selected.byteLength != null && selected.text != null && utf8ByteLength(selected.text) !== selected.byteLength ? (
                      <Tag color="warning">{t('knowledge.partial')}</Tag>
                    ) : null}
                    {selected.contentIncluded && selected.text ? (
                      <Tag
                        data-testid="digest-check"
                        color={digestCheck === 'verified' ? 'success' : digestCheck === 'mismatch' ? 'error' : 'default'}
                      >
                        {digestCheck === 'verified' ? <Stamp size={16} strokeWidth={1.5} aria-label={t('knowledge.digestVerified')} /> : null}
                        {t(DIGEST_LABEL[digestCheck])}
                      </Tag>
                    ) : null}
                  </Space>
                  {selectedDigests > 1 ? (
                    <Typography.Text type="warning" data-testid="path-digest-notice">
                      {t('knowledge.multipleDigests', { count: selectedDigests })}
                    </Typography.Text>
                  ) : null}
                  <Typography.Text type="secondary" data-testid="knowledge-citing-works">
                    {t('knowledge.citingWorks')}: {selected.citingWorks.map((work) => work.title).join(', ')}
                  </Typography.Text>
                  {selected.text && selected.contentIncluded ? (
                    <Typography.Paragraph style={{ whiteSpace: 'pre-wrap', overflowWrap: 'anywhere' }}>{selected.text}</Typography.Paragraph>
                  ) : (
                    <Typography.Text type="secondary">{t('knowledge.contentMissing')}</Typography.Text>
                  )}
                  {selected.citingWorks.map((work) => {
                    if (!work.goals?.length && !work.acceptance?.length) return null;
                    return (
                      <Card key={work.key} size="small" type="inner" title={t('knowledge.contractFor', { title: work.title })}>
                        {work.goals?.length ? (
                          <List size="small" header={t('workbench.detail.goals')} dataSource={work.goals} renderItem={(goal) => <List.Item>{rawObservationText(goal).text}</List.Item>} />
                        ) : null}
                        {work.acceptance?.length ? (
                          <List size="small" header={t('workbench.detail.acceptance')} dataSource={work.acceptance} renderItem={(item) => <List.Item>{rawObservationText(item).text}</List.Item>} />
                        ) : null}
                      </Card>
                    );
                  })}
                  <Space size={8} wrap>
                    {selected.citingWorks.map((work) => (
                      <Link key={work.key} to={`/projects/${project}?tab=plan&work=${encodeURIComponent(work.key)}`}>
                        {t('knowledge.openWork', { title: work.title })}
                      </Link>
                    ))}
                  </Space>
                </Space>
              </Card>
            </Col>
          ) : null}
        </Row>
      )}
    </Space>
  );
}
