import { Alert, Flex, List, Space, Typography } from 'antd';
import type { ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import { moreLinesText, type CappedList, type ProgressReportResult } from '../core/observation';
import { humanText } from '../i18n/human-language';
import { NotReportedYet } from './states';
import { TechnicalDetails } from './TechnicalDetails';
import { ToneBadge } from './ToneBadge';

function ReportList<T>({ title, list, testId, render }: {
  title: string;
  list: CappedList<T>;
  testId: string;
  render: (entry: T) => ReactNode;
}) {
  const { i18n } = useTranslation();
  if (list.shown.length === 0) return null;
  return (
    <section data-testid={testId}>
      <Typography.Text strong>{title}</Typography.Text>
      <List size="small" split={false} dataSource={list.shown} renderItem={(entry) => <List.Item>{render(entry)}</List.Item>} />
      {list.more > 0 ? <Typography.Text type="secondary">{moreLinesText(list.more, i18n.language)}</Typography.Text> : null}
    </section>
  );
}

/** The reporter's checkpoint in plain words; every raw field is in the collapsed technical details. */
export function ProgressReport({ report }: { report: ProgressReportResult }) {
  const { t, i18n } = useTranslation();
  if (report.kind === 'missing') return <NotReportedYet />;
  return (
    <Flex vertical gap={12} data-testid="progress-report">
      {report.contractNote ? <Alert type="warning" message={report.contractNote} data-testid="progress-report-contract" /> : null}
      {report.summary ? <Typography.Paragraph data-testid="progress-report-summary" data-reporter-text style={{ marginBottom: 0 }}>{report.summary}</Typography.Paragraph> : null}
      <ReportList title={t('progressReport.completed')} list={report.completed} testId="progress-report-completed" render={(line) => <Typography.Text data-reporter-text>{line}</Typography.Text>} />
      {report.blockers.reported ? (
        report.blockers.shown.length > 0
          ? <ReportList title={t('progressReport.blockers')} list={report.blockers} testId="progress-report-blockers" render={(line) => <Typography.Text data-reporter-text>{line}</Typography.Text>} />
          : <Typography.Text type="secondary" data-testid="progress-report-blockers">{humanText('blockersNone', i18n.language)}</Typography.Text>
      ) : null}
      <ReportList
        title={t('progressReport.tests')}
        list={report.tests}
        testId="progress-report-tests"
        render={(test) => (
          <Space wrap size={[12, 4]}>
            <Typography.Text>{test.name}</Typography.Text>
            <ToneBadge tone={test.tone} text={test.outcome} />
          </Space>
        )}
      />
      <ReportList title={t('progressReport.artifacts')} list={report.artifacts} testId="progress-report-artifacts" render={(artifact) => <Typography.Text>{artifact.text}</Typography.Text>} />
      {report.declaration.length > 0 ? (
        <Typography.Text type="secondary" data-testid="progress-report-declaration">{report.declaration.join(' · ')}</Typography.Text>
      ) : null}
      <TechnicalDetails items={report.technical.items.map((item) => ({ label: item.label, value: item.value }))} />
    </Flex>
  );
}
