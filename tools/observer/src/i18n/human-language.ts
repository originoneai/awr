/** Human-language vocabulary and forbidden visible-text patterns (no UI imports). */
export const HUMAN_VOCABULARY = {
  'zh-CN': {
    technicalDetails: '技术细节',
    copy: '复制',
    copied: '已复制',
    notMeasured: '尚未读取',
    measured: '已更新',
    stale: '可能已过期',
    failed: '读取失败',
    notProvided: '未提供',
    retry: '重试',
    partialFailure: '部分内容暂时读取不到，已保留可读内容',
    empty: '这里暂时没有内容',
    notOpen: '该功能尚未开放',
    currentLogin: '当前登录状态',
    revokeLocal: '退出登录只影响这台设备上的登录；要让其他设备或 Agent 失效，请联系管理员',
    contentVerified: '内容未被改动',
    contentMismatch: '内容对不上，请勿采信',
    noCompletion: '还没有完成记录',
    noRecord: '暂时没看到记录',
    activityReadFailed: '活动记录暂时读取不到，稍后会自动重试',
    projectReadFailed: '项目列表暂时读取不到',
  },
  'en-US': {
    technicalDetails: 'Technical details',
    copy: 'Copy',
    copied: 'Copied',
    notMeasured: 'Not read yet',
    measured: 'Updated',
    stale: 'May be outdated',
    failed: 'Read failed',
    notProvided: 'Not provided',
    retry: 'Try again',
    partialFailure: 'Some content is unavailable; confirmed content remains visible',
    empty: 'Nothing here yet',
    notOpen: 'This feature is not available yet',
    currentLogin: 'Current sign-in',
    revokeLocal: 'Signing out only affects this device. Contact an administrator to sign out other devices or agents',
    contentVerified: 'Content is unchanged',
    contentMismatch: 'Content does not match; do not trust it',
    noCompletion: 'No completion record yet',
    noRecord: 'No records found yet',
    activityReadFailed: 'Activity cannot be loaded right now; it will retry automatically',
    projectReadFailed: 'Projects cannot be loaded right now',
  },
} as const;

export type HumanLanguage = keyof typeof HUMAN_VOCABULARY;
export type HumanVocabularyKey = keyof (typeof HUMAN_VOCABULARY)['en-US'];

export function humanText(key: HumanVocabularyKey, language: string): string {
  const locale: HumanLanguage = language.toLowerCase().startsWith('zh') ? 'zh-CN' : 'en-US';
  return HUMAN_VOCABULARY[locale][key];
}

export const FORBIDDEN_VISIBLE_PATTERNS: ReadonlyArray<{ id: string; pattern: RegExp }> = [
  { id: 'snake_case', pattern: /\b[a-z]+_[a-z_]+\b/ },
  { id: 'operation_name', pattern: /\b[a-z][a-z0-9_]*(?:\.[a-z][a-z0-9_]*)+\b/ },
  { id: 'long_hex', pattern: /\b[0-9a-f]{40,}\b/i },
  { id: 'ulid', pattern: /\b[0-9A-HJKMNP-TV-Z]{26}\b/ },
  { id: 'uuid', pattern: /\b[0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}\b/i },
  { id: 'sha256', pattern: /\bsha256:/i },
  { id: 'error_class', pattern: /\b[A-Z][A-Za-z0-9]*Error\b/ },
  { id: 'engineering_word', pattern: /\b(?:claim|lease|digest|checkpoint|fence|epoch|payload|cursor|OAuth|client_id|request_id|snapshot|completion)\b/i },
  { id: 'engineering_copy', pattern: /切片|占位|合同|回执|绑定|轮号|未测量|快照/ },
  { id: 'not_measured', pattern: /\bnot measured\b/i },
];
