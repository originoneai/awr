import { useQuery, useQueryClient } from '@tanstack/react-query';
import { Alert, Button, Card, Col, Input, List, Row, Space, Tag, Typography, theme } from 'antd';
import { useEffect, useState, type ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import { ObservationEmpty, ObservationForbidden } from '../components/states';
import { documentTitle, judgeContentRead, utf8ByteLength } from '../core/observation';
import { formatObservationValue } from '../i18n/craft';
import { ApiError, isAuthError } from '../lib/api';
import { formatUnixMs } from '../lib/format';
import { projectDocumentsQueryOptions, sourceContentQueryOptions } from '../lib/queries';
import { useRevocation } from '../lib/revocation';
import { useTextDigest } from '../lib/text-digest';
import type { ProjectDocument } from '../lib/types';
import { craftSelectionStyle } from '../theme';

/** Document bodies remain opt-in and reuse the existing authorized content read. */
function DocumentText({ project, document }: { project: string; document: ProjectDocument }) {
  const { t } = useTranslation();
  const { revoked } = useRevocation();
  const [requested, setRequested] = useState(false);
  const content = useQuery({
    ...sourceContentQueryOptions(project, document.path ?? '', document.sha256 ?? '', revoked, document.content_work_id),
    enabled: requested && document.readable && !revoked,
    staleTime: 0,
    refetchOnMount: 'always',
  });
  const data = content.isFetchedAfterMount && !content.isError ? content.data?.result.data : undefined;
  const text = typeof data?.text === 'string' ? data.text : null;
  const verdict = useTextDigest(text, document.sha256);
  const status = text === null || verdict === 'pending' ? null : judgeContentRead({
    verdict, byteLength: utf8ByteLength(text), declaredByteLength: document.byte_length, more: false,
  });
  if (revoked || isAuthError(content.error)) return <ObservationForbidden />;
  if (!requested) return <Button onClick={() => setRequested(true)}>{t('knowledge.readFull')}</Button>;
  if (content.isError) return <Space orientation="vertical">
    <Typography.Text type="warning">{t('knowledge.readFailed')}</Typography.Text>
    <Button onClick={() => void content.refetch()}>{t('knowledge.retry')}</Button>
  </Space>;
  if (content.isPending || !content.isFetchedAfterMount || verdict === 'pending') return <Typography.Text type="secondary">{t('common.loading')}</Typography.Text>;
  if (text === null) return <Typography.Text type="secondary">{t('knowledge.notPlainText')}</Typography.Text>;
  if (verdict === 'mismatch' || status === 'partial') return <Typography.Text type="warning">{t('knowledge.digestMismatch')}</Typography.Text>;
  return <Space orientation="vertical" size={12} style={{ width: '100%' }}>
    <Tag color={status === 'complete' ? 'success' : 'default'}>
      {t(verdict === 'verified' ? 'knowledge.digestVerified' : 'knowledge.digestUnverified')}
    </Tag>
    <Typography.Paragraph data-testid="library-document-text" style={{ whiteSpace: 'pre-wrap', overflowWrap: 'anywhere' }}>
      {text}
    </Typography.Paragraph>
  </Space>;
}

/** A project-wide library, independent of how many work summaries are loaded. */
export function ProjectDocumentLibrary({ project, unsupportedFallback }: { project: string; unsupportedFallback?: ReactNode }) {
  const { t, i18n } = useTranslation();
  const { token } = theme.useToken();
  const queryClient = useQueryClient();
  const { revoked } = useRevocation();
  const [search, setSearch] = useState('');
  const [paging, setPaging] = useState<{ cursor: string | null; previous: Array<string | null> }>({ cursor: null, previous: [] });
  const [selection, setSelection] = useState<number | null>(null);
  const library = useQuery(projectDocumentsQueryOptions(project, search, paging.cursor, revoked));
  const expired = library.error instanceof ApiError && library.error.code === 'CursorExpired';
  // A cursor can expire after permission narrowing. Hide retained data in the same render.
  const data = library.isFetchedAfterMount && !library.isError ? library.data?.result.data : undefined;
  useEffect(() => {
    if (!expired || !paging.cursor) return;
    for (const resource of ['documents.list', 'source.content']) {
      const queryKey = ['team', resource, project];
      void queryClient.cancelQueries({ queryKey });
      queryClient.removeQueries({ queryKey });
    }
    setSelection(null);
    setPaging({ cursor: null, previous: [] });
  }, [expired, paging.cursor, project, queryClient]);
  const documents = data?.items ?? [];
  const selected = documents.find((item) => item.document_position === selection) ?? documents[0];
  const name = (item: ProjectDocument) => documentTitle(item.name, item.path, {
    language: i18n.language, position: item.document_position,
  }).text;
  const reset = () => {
    setPaging({ cursor: null, previous: [] });
    setSelection(null);
    if (!paging.cursor) void library.refetch();
  };

  if (revoked || isAuthError(library.error)) return <ObservationForbidden />;
  if (library.error instanceof ApiError && library.error.code === 'Unsupported' && unsupportedFallback) return unsupportedFallback;
  return <Space orientation="vertical" size={16} style={{ width: '100%' }} data-testid="project-document-library">
    <Card size="small">
      <Space size={16} wrap>
        <Input.Search placeholder={t('knowledge.librarySearch')} aria-label={t('knowledge.librarySearch')}
          allowClear maxLength={512} style={{ maxWidth: 320 }} onSearch={(value) => {
            setSearch(value.trim()); setPaging({ cursor: null, previous: [] }); setSelection(null);
          }} />
        {data ? <Typography.Text type="secondary">{t('knowledge.libraryCount', { count: data.total })}</Typography.Text> : null}
        <Button onClick={reset} aria-label={t('knowledge.refreshLibrary')} loading={library.isFetching}>{t('knowledge.refreshLibrary')}</Button>
      </Space>
      {data ? <Typography.Paragraph type="secondary" style={{ marginBottom: 0, marginTop: 12 }}>
        {t('knowledge.libraryUpdated', { time: formatUnixMs(data.as_of.at_unix_ms, i18n.language) })}
      </Typography.Paragraph> : null}
    </Card>
    {library.isError ? <Alert type="warning" showIcon title={t('knowledge.libraryFailed')} /> : null}
    {library.isPending || !library.isFetchedAfterMount && !library.isError ? <Typography.Text type="secondary">{t('common.loading')}</Typography.Text> : null}
    {data && documents.length === 0 ? <ObservationEmpty readAtUnixMs={data.as_of.at_unix_ms} /> : null}
    {documents.length > 0 ? <Row gutter={[16, 16]} style={{ width: '100%' }}>
      <Col xs={24} lg={8}>
        <Card size="small" title={t('knowledge.sources')}>
          <List dataSource={documents} rowKey={(item) => item.document_position} renderItem={(item) => <List.Item>
            <Button type="text" block onClick={() => setSelection(item.document_position)}
              aria-pressed={selected?.document_position === item.document_position}
              style={{ ...craftSelectionStyle(token, selected?.document_position === item.document_position),
                height: 'auto', textAlign: 'start', whiteSpace: 'normal' }}>
              <Space orientation="vertical" size={4} style={{ width: '100%' }}>
                <Typography.Text style={{ overflowWrap: 'anywhere' }}>{formatObservationValue(name(item), i18n.language)}</Typography.Text>
                <Typography.Text type="secondary">{t(`knowledge.documentStatus.${item.status}`)}</Typography.Text>
              </Space>
            </Button>
          </List.Item>} />
          <Space style={{ width: '100%', justifyContent: 'flex-end', marginTop: 12 }}>
            <Button disabled={paging.previous.length === 0} onClick={() => {
              setPaging({ cursor: paging.previous.at(-1) ?? null, previous: paging.previous.slice(0, -1) }); setSelection(null);
            }}>{t('knowledge.previousDocuments')}</Button>
            <Button disabled={!data?.next_cursor} onClick={() => {
              setPaging({ cursor: data?.next_cursor ?? null, previous: [...paging.previous, paging.cursor] }); setSelection(null);
            }}>{t('knowledge.nextDocuments')}</Button>
          </Space>
        </Card>
      </Col>
      {selected ? <Col xs={24} lg={16}>
        <Card size="small" title={formatObservationValue(name(selected), i18n.language)}>
          <Space orientation="vertical" size={16} style={{ width: '100%' }}>
            <Typography.Text type="secondary">{t('knowledge.registeredDocument')}</Typography.Text>
            {selected.byte_length !== null ? <Typography.Text type="secondary">
              {t('knowledge.documentSize', { size: new Intl.NumberFormat(i18n.language, {
                style: 'unit', unit: 'kilobyte', maximumFractionDigits: 1,
              }).format(selected.byte_length / 1024) })}
            </Typography.Text> : null}
            {selected.readable ? <DocumentText key={`${selected.path}:${selected.sha256}`} project={project} document={selected} />
              : <Typography.Text type="secondary">{t(`knowledge.documentStatus.${selected.status}`)}</Typography.Text>}
          </Space>
        </Card>
      </Col> : null}
    </Row> : null}
  </Space>;
}
