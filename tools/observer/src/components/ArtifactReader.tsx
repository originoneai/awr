import { useQuery } from '@tanstack/react-query';
import { Button, Segmented, Space, Tag, Tooltip, Typography } from 'antd';
import { useState } from 'react';
import { Stamp } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { ApiError } from '../lib/api';
import { FieldLabel, ObservationValue } from './ObservationValue';
import { fetchArtifactContent, fetchArtifactSegments, type ArtifactSelector } from '../lib/artifact';
import { asText, at } from '../lib/json';

type ReadMode = 'content' | 'segment';

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
  const [mode, setMode] = useState<ReadMode>('content');
  const artifactId = asText(at(artifact, ['query', 'artifact_id']));
  const expected = asText(at(artifact, ['query', 'expected_sha256']));
  const segmentAvailable = Boolean(at(artifact, ['segment_query', 'artifact_id']));
  const selector: ArtifactSelector | null = artifactId && expected ? {
    work_id: asText(at(artifact, ['query', 'work_id'])) ?? '',
    artifact_id: artifactId,
    expected_sha256: expected,
  } : null;
  const read = useQuery({
    queryKey: ['team', 'artifact-read', project, mode, selector],
    queryFn: ({ signal }: { signal: AbortSignal }) => mode === 'content'
      ? fetchArtifactContent(project, selector as ArtifactSelector, signal)
      : fetchArtifactSegments(project, selector as ArtifactSelector, signal),
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
      <Typography.Text strong style={{ overflowWrap: 'anywhere' }}>
        <ObservationValue value={at(artifact, ['manifest_artifact_id'])} />
      </Typography.Text>
      <Typography.Text type="secondary"><FieldLabel field="artifact_id" />: <ObservationValue value={artifactId} /></Typography.Text>
      <Typography.Text type="secondary"><FieldLabel field="byte_length" />: <ObservationValue value={at(artifact, ['byte_length'])} field="byte_length" /></Typography.Text>
      <Space size={8} wrap>
        <Tooltip title={expected}>
          <Typography.Text type="secondary">{expected ? `${expected.slice(0, 12)}…` : t('common.notProvided')}</Typography.Text>
        </Tooltip>
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
        {expanded ? (
          <Segmented
            size="small"
            value={mode}
            onChange={(value) => setMode(value as ReadMode)}
            options={[
              { label: t('artifact.content'), value: 'content' },
              { label: t('artifact.segment'), value: 'segment', disabled: !segmentAvailable },
            ]}
          />
        ) : null}
      </Space>
      {expanded && selector?.work_id ? (
        read.isPending ? <Typography.Text type="secondary">{t('common.loading')}</Typography.Text>
          : read.isError ? (
            <Typography.Text type="warning">
              {t('artifact.readFailed', { code: errorCode })}
              {errorCode === 'Unavailable' ? ` ${t('artifact.digestMayChange')}` : ''}
            </Typography.Text>
          ) : read.data ? (
            <>
              {read.data.text ? (
                <Typography.Paragraph
                  data-testid={`artifact-read-${mode}-${artifactId}`}
                  style={{ whiteSpace: 'pre-wrap', overflowWrap: 'anywhere' }}
                >
                  {read.data.text}
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
