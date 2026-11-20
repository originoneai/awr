/** The designed task route; the current placeholder can later become a full task page without changing callers. */
export function taskPath(project: string, workId: string): string {
  return `/tasks/${encodeURIComponent(workId)}?scope=${encodeURIComponent(project)}`;
}
