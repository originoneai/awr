import { useQueries, useQuery } from '@tanstack/react-query';
import { Card, Col, Input, List, Row, Space, Tag, Tooltip, Typography, theme } from 'antd';
import { useEffect, useState } from 'react';
import { Stamp } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { Link } from 'react-router';
import { ObservationEmpty, ObservationForbidden } from '../components/states';
import { isAuthError } from '../lib/api';
import { overviewQueryOptions, workSnapshotQueryOptions } from '../lib/queries';
import { useRevocation } from '../lib/revocation';
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

function textOf(value: WorkContractValue | undefined): string | null {
  if (value == null) return null;
  if (typeof value === 'string' || typeof value === 'number' || typeof value === 'boolean') return String(value);
  return JSON.stringify(value);
}

function shortSha(sha: string) {
  return sha.length > 12 ? `${sha.slice(0, 12)}…` : sha;
}

/** UTF-8 byte length, not UTF-16 code units (multibyte documents). */
function utf8Length(text: string) {
  return new TextEncoder().encode(text).length;
}

type DigestCheck = 'pending' | 'verified' | 'mismatch' | 'unverified';

/** Assemble the knowledge list from per-work reads, never a project-level read. */
export function ProjectKnowledge({ project }: { project: string }) {
  const { t } = useTranslation();
  const { token } = theme.useToken();
  const { revoked } = useRevocation();
  const overview = useQuery(overviewQueryOptions(project, revoked));
  const works = overview.data?.works ?? [];
  const snapshots = useQueries({
    queries: works.map((work) => workSnapshotQueryOptions(project, work.key, work.workstream_id, revoked)),
  });
  const [filter, setFilter] = useState('');
  const [selectedKey, setSelectedKey] = useState<string | null>(null);
  const [digestCheck, setDigestCheck] = useState('unverified' as DigestCheck);

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

  const selectedText = selected?.text;
  const selectedSha256 = selected?.sha256;
  const selectedContentIncluded = selected?.contentIncluded;
  useEffect(() => {
    // Verify the displayed text against its declared digest with Web Crypto;
    // without crypto.subtle the badge must read unverified, never verified.
    if (!selectedText || !selectedContentIncluded || !selectedSha256) {
      setDigestCheck('unverified');
      return;
    }
    if (typeof crypto === 'undefined' || !crypto.subtle) {
      setDigestCheck('unverified');
      return;
    }
    let cancelled = false;
    setDigestCheck('pending');
    crypto.subtle.digest('SHA-256', new TextEncoder().encode(selectedText))
      .then((buffer) => {
        if (cancelled) return;
        const hex = Array.from(new Uint8Array(buffer)).map((b) => b.toString(16).padStart(2, '0')).join('');
        setDigestCheck(hex === selectedSha256 ? 'verified' : 'mismatch');
      })
      .catch(() => {
        if (!cancelled) setDigestCheck('unverified');
      });
    return () => { cancelled = true; };
  }, [selectedText, selectedSha256, selectedContentIncluded]);

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
                    style={{
                      cursor: 'pointer',
                      background: selected && `${selected.path}::${selected.sha256}` === `${entry.path}::${entry.sha256}` ? token.colorFillSecondary : undefined,
                      border: selected && `${selected.path}::${selected.sha256}` === `${entry.path}::${entry.sha256}` ? `1px solid ${token.colorPrimary}` : undefined,
                      borderRadius: selected && `${selected.path}::${selected.sha256}` === `${entry.path}::${entry.sha256}` ? token.borderRadius : undefined,
                    }}
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
                    {selected.byteLength != null && selected.text != null && utf8Length(selected.text) !== selected.byteLength ? (
                      <Tag color="warning">{t('knowledge.partial')}</Tag>
                    ) : null}
                    {selected.contentIncluded && selected.text ? (
                      <Tag
                        data-testid="digest-check"
                        color={digestCheck === 'verified' ? 'success' : digestCheck === 'mismatch' ? 'error' : 'default'}
                      >
                        {digestCheck === 'verified' ? <Stamp size={16} strokeWidth={1.5} aria-label={t('knowledge.digestVerified')} /> : null}
                        {digestCheck === 'verified'
                          ? t('knowledge.digestVerified')
                          : digestCheck === 'mismatch'
                            ? t('knowledge.digestMismatch')
                            : t('knowledge.digestUnverified')}
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
                          <List size="small" header={t('workbench.detail.goals')} dataSource={work.goals} renderItem={(goal) => <List.Item>{textOf(goal)}</List.Item>} />
                        ) : null}
                        {work.acceptance?.length ? (
                          <List size="small" header={t('workbench.detail.acceptance')} dataSource={work.acceptance} renderItem={(item) => <List.Item>{textOf(item)}</List.Item>} />
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
