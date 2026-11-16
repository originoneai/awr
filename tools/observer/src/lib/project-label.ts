import type { TeamProject } from './types';
import { workLabel } from '../core/observation';
import i18n from '../i18n';

/** Live discovery guarantees the project key; display metadata is optional. */
export function projectLabel(project: TeamProject, position = 1): string {
  for (const name of [project.title, project.key]) {
    const label = workLabel(name?.trim(), { language: i18n.language });
    if (label.kind === 'named') return label.text;
  }
  return i18n.t('shell.unnamedProject', { number: position });
}
