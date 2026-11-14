import { useQuery } from '@tanstack/react-query';
import { Button, Space, Tag, Typography } from 'antd';
import { useState } from 'react';
import { Stamp } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { ApiError } from '../lib/api';
import { fetchArtifactContent, fetchArtifactSegments, type ArtifactSelector } from '../lib/artifact';
import { asText, at } from '../lib/json';

function isOperationalRecord(text: string): boolean {
  try {
    const record = JSON.parse(text);
    return record != null && typeof record === 'object'
      && [record.codec, record.protocol].some(value => typeof value === 'string' && value.startsWith('awr-'));
  } catch {
    return false;
  }
}

/** On-demand artifact bytes; verification is derived atomically with the bytes. */
export function ArtifactReader({
  project,
  artifact,
  revoked,
}: {
  project: string;
  artifact: unknown;
  revoked: boolean;
}) {
  const { t } = useTranslation();
  const [expanded, setExpanded] = useState(false);
  const artifactId = asText(at(artifact, ['query', 'artifact_id']));
  const expected = asText(at(artifact, ['query', 'expected_sha256']));
  const segmentAvailable = Boolean(at(artifact, ['segment_query', 'artifact_id']));
  const selector: ArtifactSelector | null = artifactId && expected ? {
    work_id: asText(at(artifact, ['query', 'work_id'])) ?? '',
    artifact_id: artifactId,
    expected_sha256: expected,
  } : null;
  const read = useQuery({
    queryKey: ['team', 'artifact-read', project, selector],
    queryFn: async ({ signal }: { signal: AbortSignal }) => {
      try {
        return await fetchArtifactContent(project, selector as ArtifactSelector, signal);
      } catch (error) {
        if (!(error instanceof ApiError) || error.code !== 'Unavailable' || !segmentAvailable) throw error;
        return fetchArtifactSegments(project, selector as ArtifactSelector, signal);
      }
    },
    enabled: expanded && !revoked && Boolean(selector?.work_id),
    // Digest-addressed bytes are immutable: one read per selector, never a poll.
    refetchInterval: false,
    refetchOnWindowFocus: false,
    staleTime: Infinity,
    retry: (failureCount, error) => !(error instanceof ApiError && error.code === 'Unavailable') && failureCount < 1,
  });
  const status = read.data?.status;
  const digestText = status === 'verified' ? t('artifact.verified')
    : status === 'mismatch' ? t('artifact.mismatch') : t('acceptance.unverified');
  const errorCode = read.error instanceof ApiError ? read.error.code : 'UnknownError';

  return (
    <Space direction="vertical" size={6} style={{ width: '100%' }}>
      <Space size={8} wrap>
        {status ? (
          <Tag data-testid={`artifact-digest-${artifactId}`} color={status === 'verified' ? 'success' : status === 'mismatch' ? 'error' : 'default'}>
            {status === 'verified' ? <Stamp size={16} strokeWidth={1.5} aria-label={digestText} /> : null}
            {digestText}
          </Tag>
        ) : <Tag>{t('acceptance.unverified')}</Tag>}
        {selector?.work_id ? (
          <Button size="small" data-testid={`artifact-expand-${artifactId}`} onClick={() => setExpanded((value) => !value)}>
            {expanded ? t('artifact.collapse') : t('artifact.expand')}
          </Button>
        ) : null}
      </Space>
      {expanded && selector?.work_id ? (
        read.isPending ? <Typography.Text type="secondary">{t('common.loading')}</Typography.Text>
          : read.isError ? (
            <Typography.Text type="warning">
              {t('artifact.readFailed')}
              {errorCode === 'Unavailable' ? ` ${t('artifact.digestMayChange')}` : ''}
            </Typography.Text>
          ) : read.data ? (
            <>
              {read.data.text ? (
                <Typography.Paragraph
                  data-testid={`artifact-read-content-${artifactId}`}
                  style={{ whiteSpace: 'pre-wrap', overflowWrap: 'anywhere' }}
                >
                  {isOperationalRecord(read.data.text) ? t('artifact.recordSaved') : read.data.text}
                </Typography.Paragraph>
              ) : (
                <Typography.Text type={status === 'mismatch' ? 'warning' : 'secondary'}>
                  {status === 'mismatch' ? t('artifact.mismatch') : t('artifact.missing')}
                </Typography.Text>
              )}
            </>
          ) : null
      ) : null}
    </Space>
  );
}
