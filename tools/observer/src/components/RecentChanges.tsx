import { List, Space, Tag, Typography } from 'antd';
import { useMemo } from 'react';
import { useTranslation } from 'react-i18next';
import { Link } from 'react-router';
import { ageText, foldOperations, operationWord } from '../core/observation';
import type { TeamActivityItem } from '../lib/types';
import { TechnicalDetails } from './TechnicalDetails';

/** The pause after which two records no longer read as one stretch of work (owner decision). */
const PAUSE_THRESHOLD_MS = 30 * 60 * 1000;

/**
 * The newest stretches of activity in plain words: consecutive operations of one kind fold into a
 * count ("3 reads"), a lone operation says what it was, and a stretch that includes an unknown
 * result says so. Every record is still counted: the raw actions and audit ids of the shown steps
 * are in the collapsed technical details.
 */
export function RecentChanges({
  items,
  maxSteps,
  viewAllHref,
  testId,
}: {
  items: TeamActivityItem[];
  maxSteps: number;
  viewAllHref: string;
  testId?: string;
}) {
  const { t, i18n } = useTranslation();
  const language = i18n.language;
  const folded = useMemo(() => foldOperations(items, { language, gapThresholdMs: PAUSE_THRESHOLD_MS }), [items, language]);
  const byId = useMemo(
    () => new Map(folded.uniqueEvents.flatMap((event) => (event.id ? [[event.id, event] as const] : []))),
    [folded],
  );
  const steps = folded.steps.slice(0, maxSteps);
  const notes = Array.from(new Map(
    folded.disclosures.filter((item) => item.code !== 'time-gap').map((item) => [item.code, item]),
  ).values());
  const nowUnixMs = Date.now();
  const timeZone = Intl.DateTimeFormat().resolvedOptions().timeZone;
  return (
    <div data-testid={testId}>
      <List
        size="small"
        dataSource={steps}
        renderItem={(step, index) => {
          const sole = step.count === 1 ? operationWord(step.technical.rawActions[0], { language }).text : null;
          const unknown = step.technical.eventIds.some((id) => id != null && byId.get(id)?.result === 'unknown');
          return (
            <List.Item data-testid={`recent-step-${index}`}>
              <Space wrap size={[12, 4]}>
                <Typography.Text type="secondary" data-testid="recent-step-time">
                  {ageText(step.lastAtUnixMs, { language, nowUnixMs, timeZone }).text}
                </Typography.Text>
                <Typography.Text data-testid="recent-step-text">{sole ?? step.text}</Typography.Text>
                {unknown ? <Tag color="warning" data-testid="recent-step-unknown">{t('recentChanges.unknown')}</Tag> : null}
              </Space>
            </List.Item>
          );
        }}
      />
      {folded.steps.length > steps.length ? (
        <Typography.Paragraph type="secondary" data-testid="recent-changes-more">{t('recentChanges.more')}</Typography.Paragraph>
      ) : null}
      {notes.map((note) => (
        <Typography.Paragraph key={note.code} type="secondary" style={{ marginBottom: 4 }} data-testid={`recent-note-${note.code}`}>
          {note.text}
        </Typography.Paragraph>
      ))}
      <Link to={viewAllHref} data-testid="recent-changes-all">{t('recentChanges.viewAll')} →</Link>
      <TechnicalDetails
        items={steps.map((step, index) => ({
          label: `step ${index + 1}`,
          value: `${step.technical.rawActions.map((action) => action ?? 'null').join(', ')} | ${step.technical.eventIds.map((id) => id ?? 'null').join(', ')}`,
          testId: `recent-step-technical-${index}`,
        }))}
      />
    </div>
  );
}
