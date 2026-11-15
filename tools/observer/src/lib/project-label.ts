import type { TeamProject } from './types';
import { documentTitle } from '../core/observation';
import i18n from '../i18n';

/** Live discovery guarantees the project key; display metadata is optional. */
export function projectLabel(project: TeamProject): string {
  const label = documentTitle(project.title, null, { language: i18n.language, position: 1 });
  return label.kind === 'named' ? label.text : i18n.t('shell.unnamedProject');
}
