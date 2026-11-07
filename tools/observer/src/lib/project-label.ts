import type { TeamProject } from './types';

/** Live discovery guarantees the project key; display metadata is optional. */
export function projectLabel(project: TeamProject, includeKey = false): string {
  const title = project.title?.trim();
  return title ? (includeKey ? `${title} (${project.key})` : title) : project.key;
}
