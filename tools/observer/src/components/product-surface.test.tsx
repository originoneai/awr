import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it } from 'vitest';
import i18n from '../i18n';
import { FieldLabel, ObservationValue } from './ObservationValue';
import { ObservationForbidden, PartialFailure } from './states';
import { RecordedTime } from './RecordedTime';

afterEach(async () => { await i18n.changeLanguage('zh-CN'); });

describe('product-only observation values', () => {
  it.each(['zh-CN', 'en-US'])('keeps field names and raw statuses out of hover content in %s', async (language) => {
    await i18n.changeLanguage(language);
    const user = userEvent.setup();
    render(<><FieldLabel field="runtime_state" /><ObservationValue value="in_progress" field="runtime_state" testId="status" /></>);
    await user.hover(screen.getByTestId('status'));
    expect(document.body.textContent).not.toContain('runtime_state');
    expect(document.body.textContent).not.toContain('in_progress');
    expect(document.querySelector('[role="tooltip"]')).toBeNull();
  });

  it.each(['zh-CN', 'en-US'])('explains an unknown status without exposing its protocol value in %s', async (language) => {
    await i18n.changeLanguage(language);
    render(<ObservationValue value="internal_future_state" field="runtime_state" />);
    expect(document.body.textContent).not.toContain('internal_future_state');
    expect(document.body.textContent).toContain(language === 'zh-CN' ? '未识别状态' : 'Unrecognized state');
  });

  it('keeps the raw error in the backend while explaining a partial read', () => {
    render(<PartialFailure errorCode="InternalStorageError"><p>已确认的成果</p></PartialFailure>);
    expect(document.body.textContent).toContain('已确认的成果');
    expect(document.body.textContent).not.toContain('InternalStorageError');
    expect(document.querySelector('details,[data-testid="technical-details"]')).toBeNull();
    expect(screen.queryByRole('button', { name: /技术细节|Technical details/ })).toBeNull();
  });

  it('explains access loss using product language', () => {
    render(<ObservationForbidden />);
    expect(document.body.textContent).toContain('暂时无法查看');
    expect(document.body.textContent).not.toContain('403');
  });

  it('shows a readable time without a raw timestamp tooltip', async () => {
    const user = userEvent.setup();
    const { container } = render(<RecordedTime value={1790000000000} />);
    await user.hover(container.firstElementChild as HTMLElement);
    expect(document.body.textContent).not.toContain('1790000000000');
    expect(document.querySelector('[role="tooltip"]')).toBeNull();
  });
});
