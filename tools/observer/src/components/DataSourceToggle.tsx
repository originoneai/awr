import { Button, Popover } from 'antd';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { LayersIcon } from './icons/prototype-icons';

/** Where the open pages read from, as the bridge reports it: 'demo' or 'live'; null while
 * the fact has not been established. The control only explains that fact; it submits nothing. */
export type ShellSource = 'demo' | 'live' | null;

/** Prototype .top > .tog: the layers glyph beside a 13px label in a hairline field. The
 * product control opens an explanation instead of the prototype's demo-only source badges. */
export function DataSourceToggle({ source }: { source: ShellSource }) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const sentence = source === 'demo' ? t('shell.dataSourceDemo')
    : source === 'live' ? t('shell.dataSourceLive') : t('shell.dataSourceUnknown');
  return (
    <Popover
      trigger="click"
      placement="bottomRight"
      open={open}
      onOpenChange={setOpen}
      content={(
        <div className="shell-data-source-explanation" data-testid="data-source-explanation">
          <p>{sentence}</p>
          <p>{t('shell.dataSourceReadOnly')}</p>
        </div>
      )}
    >
      <Button
        className="shell-data-source"
        data-testid="data-source-trigger"
        aria-expanded={open}
        icon={<LayersIcon size={16} strokeWidth={1.5} aria-hidden />}
      >
        <span className="shell-data-source-label">{t('shell.dataSource')}</span>
      </Button>
    </Popover>
  );
}
