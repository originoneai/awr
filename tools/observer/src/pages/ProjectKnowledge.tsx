import { useQuery } from '@tanstack/react-query';
import { Button, Card, Col, Input, List, Pagination, Row, Space, Tag, Typography, theme } from 'antd';
import { useState } from 'react';
import { Stamp } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { Link, useSearchParams } from 'react-router';
import { ObservationEmpty, ObservationForbidden } from '../components/states';
import { TechnicalDetails } from '../components/TechnicalDetails';
import {
  documentLabel,
  judgeContentRead,
  rawObservationText,
  utf8ByteLength,
  type ContentReadStatus,
} from '../core/observation';
import { isAuthError } from '../lib/api';
import { OBSERVATION_PAGE_SIZE, validObservationPage } from '../lib/pagination';
import { contractGroups } from '../lib/contract-groups';
import { overviewQueryOptions, readableSourceRef, sourceContentQueryOptions } from '../lib/queries';
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

/** One version of one document, as the works that cite it name it. Every field the service may leave out is optional. */
interface SourceEntry {
  key: string;
  path: string | null;
  sha256: string | null;
  byteLength?: number;
  contentIncluded: boolean;
  /** A document the works must follow (it carries text), rather than a reference they may read. */
  isSpec: boolean;
  text?: string;
  citingWorks: CitingWork[];
}

/** The words of the digest check; a check that has not finished reads as not checked. */
const DIGEST_LABEL = {
  verified: 'knowledge.digestVerified',
  mismatch: 'knowledge.digestMismatch',
  unverified: 'knowledge.digestUnverified',
  pending: 'knowledge.digestUnverified',
} as const satisfies Record<DigestCheck, string>;

/** Assemble the knowledge list from per-work reads, never a project-level read. */
export function ProjectKnowledge({ project }: { project: string }) {
  const { t, i18n } = useTranslation();
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
      path: string | null;
      sha256: string | null;
      byte_length?: number;
      text?: string;
      contentIncluded: boolean;
      isSpec: boolean;
    }> = [
      ...(data.required_specs ?? []).map((spec) => ({
        path: spec.path ?? null,
        sha256: spec.sha256 ?? null,
        byte_length: spec.byte_length,
        text: spec.text,
        contentIncluded: spec.content_included !== false,
        isSpec: true,
      })),
      ...(data.authorized_readable_refs ?? []).map((ref) => ({
        path: ref.path ?? null,
        sha256: ref.sha256 ?? null,
        byte_length: ref.byte_length,
        contentIncluded: false,
        isSpec: false,
      })),
    ];
    for (const source of sources) {
      const key = `${source.path ?? ''}::${source.sha256 ?? ''}`;
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
        if (source.isSpec) existing.isSpec = true;
      } else {
        entries.set(key, {
          key,
          path: source.path,
          sha256: source.sha256,
          byteLength: source.byte_length,
          contentIncluded: source.contentIncluded,
          isSpec: source.isSpec,
          text: source.text,
          citingWorks: [citing],
        });
      }
    }
  });
  const allEntries = Array.from(entries.values());
  const names = new Map(allEntries.map((entry, index) => [entry.key, documentLabel(entry.path, { language: i18n.language, position: index + 1 }).text]));
  const needle = filter.toLowerCase();
  const visibleEntries = allEntries.filter((entry) => (
    (entry.path ?? '').toLowerCase().includes(needle) || (names.get(entry.key) ?? '').toLowerCase().includes(needle)
  ));
  const selected = visibleEntries.find((entry) => entry.key === selectedKey) ?? visibleEntries[0];
  const loadedCount = snapshots.filter((query) => query.data).length;
  const otherVersions = selected ? allEntries.filter((entry) => entry.path === selected.path) : [];
  const groups = [
    { id: 'specs', title: t('knowledge.groupSpecs'), entries: visibleEntries.filter((entry) => entry.isSpec) },
    { id: 'refs', title: t('knowledge.groupRefs'), entries: visibleEntries.filter((entry) => !entry.isSpec) },
  ].filter((group) => group.entries.length > 0);

  // The displayed text is verified against its declared digest; without Web Crypto the badge reads
  // not checked, never verified.
  const digestCheck = useTextDigest(selected?.contentIncluded ? selected.text : null, selected?.sha256);

  // Read the full text only after a person asks for it, only once per (project, path, digest),
  // never polled, never refetched on focus, and silently gone once access is revoked.
  const [requestedKeys, setRequestedKeys] = useState<ReadonlySet<string>>(() => new Set());
  const selectedTruncated = Boolean(
    selected?.text && selected.byteLength != null && utf8ByteLength(selected.text) !== selected.byteLength,
  );
  const needsFullRead = Boolean(selected) && (!selected.contentIncluded || selectedTruncated);
  const refReadable = readableSourceRef(selected?.path, selected?.sha256);
  const readRequested = Boolean(selected) && requestedKeys.has(selected.key);
  const contentRead = useQuery({
    ...sourceContentQueryOptions(
      project,
      selected?.path ?? '',
      selected?.sha256 ?? '',
      revoked,
    ),
    enabled: readRequested && refReadable && !revoked,
  });
  const readData = contentRead.data?.result?.data;
  const readText = typeof readData?.text === 'string' ? readData.text : null;
  const readVerdict = useTextDigest(readRequested && readText != null ? readText : null, selected?.sha256);
  // A 503 is a failed read, never a confirmed change: only the two digests in hand can say changed,
  // and only when the held extent does not already explain the difference.
  // While the digest check is still running the text is not shown at all: a text that turns
  // out not to match must never be on the page, not even for one render.
  const readStatus: ContentReadStatus | 'not-text' | 'checking' | null = contentRead.data
    ? (readText == null
        ? 'not-text'
        : readVerdict === 'pending'
          ? 'checking'
          : judgeContentRead({
            verdict: readVerdict,
            byteLength: utf8ByteLength(readText),
            declaredByteLength: selected?.byteLength ?? null,
            more: false,
          }))
    : null;
  const showingReadResult = readRequested && (contentRead.data != null || contentRead.isError);

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
              {groups.map((group) => (
                <List
                  key={group.id}
                  size="small"
                  header={<Typography.Text type="secondary">{group.title}</Typography.Text>}
                  dataSource={group.entries}
                  rowKey={(entry) => entry.key}
                  renderItem={(entry) => (
                    <List.Item
                      data-testid={`knowledge-source-${entry.path ?? ''}-${(entry.sha256 ?? '').slice(0, 12)}`}
                      onClick={() => setSelectedKey(entry.key)}
                      style={craftSelectionStyle(token, Boolean(selected && selected.key === entry.key))}
                    >
                      <Space direction="vertical" size={2} style={{ minWidth: 0 }}>
                        <Typography.Text strong style={{ overflowWrap: 'anywhere' }}>{names.get(entry.key)}</Typography.Text>
                        <Typography.Text type="secondary" style={{ fontSize: 12 }}>
                          {t('knowledge.citedBy', { count: entry.citingWorks.length })}
                        </Typography.Text>
                      </Space>
                    </List.Item>
                  )}
                />
              ))}
            </Card>
          </Col>
          {selected ? (
            <Col xs={24} lg={16}>
              <Card size="small" style={{ width: '100%', minWidth: 0 }} title={<Typography.Text strong style={{ overflowWrap: 'anywhere' }}>{names.get(selected.key)}</Typography.Text>}>
                <Space direction="vertical" size={12} style={{ width: '100%' }}>
                  {showingReadResult ? (
                    <Space size={8} wrap data-testid="read-result">
                      {contentRead.isError ? (
                        <>
                          <Tag color="error" data-testid="read-failed">{t('knowledge.readFailed')}</Tag>
                          <Button
                            size="small"
                            data-testid="read-retry"
                            onClick={() => void contentRead.refetch()}
                          >
                            {t('knowledge.retry')}
                          </Button>
                        </>
                      ) : readStatus === 'checking' ? null
                      : readStatus === 'not-text' ? (
                        <Tag>{t('knowledge.notPlainText')}</Tag>
                      ) : readStatus === 'changed' ? (
                        <Tag color="error">{t('knowledge.digestMismatch')}</Tag>
                      ) : readStatus === 'partial' ? (
                        // A partial read's digest check says nothing (the front part is not the
                        // whole file), so only the honest extent note shows, never "do not rely".
                        <Tag color="warning">{t('knowledge.partial')}</Tag>
                      ) : (
                        <Tag data-testid="read-digest" color={readVerdict === 'verified' ? 'success' : 'default'}>
                          {readVerdict === 'verified' ? <Stamp size={16} strokeWidth={1.5} aria-label={t('knowledge.digestVerified')} /> : null}
                          {t(DIGEST_LABEL[readVerdict === 'pending' ? 'pending' : readVerdict])}
                        </Tag>
                      )}
                    </Space>
                  ) : (
                    <Space size={8} wrap>
                      <Tag color={selected.contentIncluded ? 'success' : 'default'}>
                        {selected.contentIncluded ? t('knowledge.contentIncluded') : t('knowledge.contentMissing')}
                      </Tag>
                      {selectedTruncated ? (
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
                      {needsFullRead && !refReadable ? (
                        <Typography.Text type="secondary" data-testid="read-unavailable">
                          {t('knowledge.unreadableRef')}
                        </Typography.Text>
                      ) : null}
                      {needsFullRead && refReadable ? (
                        <Button
                          size="small"
                          data-testid="read-full"
                          // A not-yet-requested read is also "pending" in the query cache;
                          // only a request actually in flight shows as loading.
                          loading={readRequested && contentRead.isPending}
                          onClick={() => {
                            setRequestedKeys((keys) => new Set(keys).add(selected.key));
                          }}
                        >
                          {t('knowledge.readFull')}
                        </Button>
                      ) : null}
                    </Space>
                  )}
                  {otherVersions.length > 1 ? (
                    <Typography.Text type="warning" data-testid="path-digest-notice">
                      {t('knowledge.multipleVersions', { count: otherVersions.length })}
                    </Typography.Text>
                  ) : null}
                  <Typography.Text type="secondary" data-testid="knowledge-citing-works">
                    {t('knowledge.citingWorks')}:{' '}
                    {selected.citingWorks.map((work, index) => (
                      <span key={work.key}>
                        {index > 0 ? ' · ' : ''}
                        <Link to={`/projects/${project}?tab=plan&work=${encodeURIComponent(work.key)}`}>{work.title}</Link>
                      </span>
                    ))}
                  </Typography.Text>
                  {showingReadResult && readText != null && (readStatus === 'complete' || readStatus === 'partial' || readStatus === 'unverified') ? (
                    <Typography.Paragraph data-testid="read-text" style={{ whiteSpace: 'pre-wrap', overflowWrap: 'anywhere' }}>{readText}</Typography.Paragraph>
                  ) : null}
                  {!showingReadResult && selected.text && selected.contentIncluded ? (
                    <Typography.Paragraph style={{ whiteSpace: 'pre-wrap', overflowWrap: 'anywhere' }}>{selected.text}</Typography.Paragraph>
                  ) : null}
                  {readStatus === 'checking' ? (
                    <Typography.Text type="secondary" data-testid="read-checking">{t('common.loading')}</Typography.Text>
                  ) : null}
                  {contractGroups(selected.citingWorks).map((group) => (
                    <Card key={group.titles.join('\u0000')} size="small" type="inner" title={t('knowledge.contractFor', { title: group.titles.join(t('knowledge.titleSeparator')) })}>
                      {group.goals?.length ? (
                        <List size="small" header={t('workbench.detail.goals')} dataSource={group.goals} renderItem={(goal) => <List.Item>{rawObservationText(goal).text}</List.Item>} />
                      ) : null}
                      {group.acceptance?.length ? (
                        <List size="small" header={t('workbench.detail.acceptance')} dataSource={group.acceptance} renderItem={(item) => <List.Item>{rawObservationText(item).text}</List.Item>} />
                      ) : null}
                    </Card>
                  ))}
                  <TechnicalDetails
                    items={[
                      { label: 'path', value: selected.path },
                      { label: 'sha256', value: selected.sha256 },
                      { label: 'byte_length', value: selected.byteLength },
                      { label: 'content_included', value: selected.contentIncluded },
                      { label: 'digest_check', value: digestCheck },
                      { label: 'citing_works', value: selected.citingWorks.map((work) => work.key) },
                      ...(otherVersions.length > 1 ? [{ label: 'versions_of_this_path', value: otherVersions.map((entry) => entry.sha256) }] : []),
                      ...(readRequested ? [
                        { label: 'read_status', value: contentRead.isError ? 'failed' : readStatus },
                        { label: 'read_error_code', value: (contentRead.error as { code?: string } | null)?.code ?? null },
                        { label: 'read_received_sha256', value: readData?.sha256 ?? null },
                        { label: 'read_received_byte_length', value: readData?.byte_length ?? null },
                      ] : []),
                    ]}
                  />
                </Space>
              </Card>
            </Col>
          ) : null}
        </Row>
      )}
    </Space>
  );
}
