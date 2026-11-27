import { settle } from './shared.mjs';

export async function documentDetail(page) {
  await page.getByTestId('project-document-library').waitFor();
  const read = page.getByRole('button', { name: /读取全文|Read full/ });
  if (await read.count()) {
    await read.first().click();
    await page.getByTestId('library-document-text').waitFor();
  }
  await settle(page);
}

export default {
  name: 'project-knowledge',
  routeActions: {
    'project-knowledge': documentDetail,
  },
};
