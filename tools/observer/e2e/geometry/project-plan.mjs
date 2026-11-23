import { settle } from './shared.mjs';

export async function planDetail(page) {
  await page.getByTestId('work-plan-page').waitFor();
  await page.getByTestId('plan-status-T-101').waitFor().catch(() => {});
  await settle(page);
}

export default {
  name: 'project-plan',
  routeActions: {
    'project-plan': planDetail,
  },
};
