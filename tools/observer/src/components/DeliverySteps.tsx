import { Steps, Typography } from 'antd';

export type DeliveryTone = 'positive' | 'warning' | 'negative' | 'neutral' | 'accent';
export interface DeliveryStep {
  key: string;
  title: string;
  text: string;
  tone: DeliveryTone;
  testId?: string;
}

/** Prototype-shaped five-step delivery status, with no service-state inference. */
export function DeliverySteps({ steps, testId }: { steps: DeliveryStep[]; testId: string }) {
  return (
    <Steps
      data-testid={testId}
      orientation="horizontal"
      labelPlacement="vertical"
      size="small"
      items={steps.map((step) => ({
        key: step.key,
        title: <Typography.Text>{step.title}</Typography.Text>,
        content: (
          <Typography.Text data-testid={step.testId} type={step.tone === 'positive' ? 'success' : step.tone === 'negative' ? 'danger' : step.tone === 'warning' ? 'warning' : 'secondary'}>
            {step.text}
          </Typography.Text>
        ),
        status: step.tone === 'positive' ? 'finish' : step.tone === 'negative' ? 'error' : step.tone === 'neutral' ? 'wait' : 'process',
      }))}
    />
  );
}
