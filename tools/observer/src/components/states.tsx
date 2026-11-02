import { Alert, Empty, Result, Typography } from 'antd';
import type { ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import { humanText } from '../i18n/human-language';
import { formatUnixMs } from '../lib/format';
import { TechnicalDetails } from './TechnicalDetails';

/**
 * Empty means "nothing observed as of a specific read", never "no data ever".
 * The read time is part of the statement.
 */
export function ObservationEmpty({ readAtUnixMs }: { readAtUnixMs?: number }) {
  const { t, i18n } = useTranslation();
  return (
    <Empty
      data-testid="state-empty"
      description={
        <>
          <Typography.Text strong>{t('state.empty.title')}</Typography.Text>
          <br />
          <Typography.Text type="secondary">
            {readAtUnixMs != null
              ? t('state.empty.description', { time: formatUnixMs(readAtUnixMs, i18n.language) })
              : t('state.empty.noRead')}
          </Typography.Text>
        </>
      }
    />
  );
}

/** 403: identity or permission revoked; restricted content is already cleared. */
export function ObservationForbidden() {
  const { t } = useTranslation();
  return (
    <Result
      data-testid="state-forbidden"
      // antd always renders its stock illustration for status="403".
      // Keep the factual title while using the neutral, icon-free variant.
      status="info"
      icon={null}
      title="403"
      subTitle={t('state.forbidden.subtitle')}
    />
  );
}

/**
 * A page or tab whose feature has not been built: it says so once. It is not an empty read ("no records were
 * observed" would read as "nothing is waiting for you"), so it never shows the empty-state block or a read time.
 */
export function NotOpenYet({ testId }: { testId?: string }) {
  const { t } = useTranslation();
  return (
    <div data-testid={testId ?? 'state-not-open'}>
      <Typography.Text strong>{t('state.placeholder.title')}</Typography.Text>
      <Typography.Paragraph type="secondary" style={{ marginBottom: 0 }}>
        {t('state.placeholder.description')}
      </Typography.Paragraph>
    </div>
  );
}

/** The report plane is not_measured: distinct from both empty and failed. */
export function NotReportedYet() {
  const { t } = useTranslation();
  return (
    <Empty
      data-testid="state-not-reported"
      description={
        <>
          <Typography.Text strong>{t('state.notReported.title')}</Typography.Text>
          <br />
          <Typography.Text type="secondary">{t('state.notReported.description')}</Typography.Text>
        </>
      }
    />
  );
}

/**
 * Partial failure: confirmed facts stay on screen with their timestamps, next
 * to an Alert naming the failed part — the failure is declared, never hidden.
 */
export function PartialFailure({
  errorCode,
  confirmedAtUnixMs,
  children,
}: {
  errorCode?: string;
  confirmedAtUnixMs?: number;
  children: ReactNode;
}) {
  const { t, i18n } = useTranslation();
  return (
    <div data-testid="state-partial">
      <Alert
        type="warning"
        showIcon
        message={t('state.partial.alert')}
        description={
          <>
            {humanText('partialFailure', i18n.language)}
            <TechnicalDetails items={[{ label: 'Error', value: errorCode ?? 'UnknownError' }]} />
            {confirmedAtUnixMs != null
              ? ` ${t('state.partial.confirmedAt', { time: formatUnixMs(confirmedAtUnixMs, i18n.language) })}`
              : ''}
          </>
        }
        style={{ marginBottom: 16 }}
      />
      {children}
    </div>
  );
}
