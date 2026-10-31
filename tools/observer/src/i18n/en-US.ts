import type zhCN from './zh-CN';

type Messages = typeof zhCN;

const enUS: Messages = {
  app: {
    wordmark: 'AWR',
    title: 'Observer',
  },
  nav: {
    workbench: 'Workbench',
    projects: 'Projects',
    inbox: 'Inbox',
    knowledge: 'Knowledge & Skills',
    members: 'Members & Agents',
    activity: 'Activity & Audit',
    settings: 'Administration',
    account: 'Account & Connection',
    advanced: 'Advanced tools',
    groupMain: 'Observe',
    groupAdmin: 'Admin',
  },
  shell: {
    scope: 'Example org',
    searchPlaceholder: 'Search projects, work items…',
    notifications: 'Notifications',
    language: 'Language',
    role: 'Observer',
  },
  project: {
    settings: 'Project settings',
    metaKey: 'Project key',
    metaRole: 'Role',
    tabs: {
      overview: 'Overview',
      plan: 'Work plan',
      deps: 'Dependencies',
      acceptance: 'Outcomes & Acceptance',
      knowledge: 'Project knowledge',
    },
  },
  freshness: {
    title: 'Freshness',
    plane: {
      report_received: 'Report received',
      authoritative: 'Authoritative',
      page_read: 'Page read',
    },
    state: {
      measured: 'measured',
      stale: 'stale',
      failed: 'failed',
      not_measured: 'not measured',
    },
  },
  state: {
    empty: {
      title: 'No records observed',
      description: 'As of the read at {{time}}, no related records were observed.',
      noRead: 'No read has completed yet; no related records were observed.',
    },
    forbidden: {
      subtitle: 'You are not authorized to view this scope; restricted content was cleared.',
    },
    notReported: {
      title: 'Not reported yet',
      description: 'No progress report has been received for this scope; that is not the same as no progress.',
    },
    partial: {
      alert: 'Partial read failure',
      description: 'The content below was confirmed before the read failed; failure: {{code}}.',
      confirmedAt: 'Confirmed at {{time}}',
    },
    placeholder: {
      title: 'Placeholder page',
      description: 'This observation view ships in a later slice; no business data is shown here.',
    },
  },
  coverage: {
    summary: 'scope {{scope}} · succeeded {{succeeded}} · failed {{failed}} · works {{enumerated}}/{{total}}',
    totalUnknown: 'unknown',
  },
  account: {
    title: 'Account & Connection',
    login: 'Sign in',
    loginBearer: 'Access token',
    loginSubmit: 'Sign in',
    logout: 'Sign out',
    logoutSubmit: 'Sign out',
    connect: 'Connect my agent',
    connectSubmit: 'Connect',
    revoke: 'Revoke connection',
    revokeSubmit: 'Revoke all connections',
    done: 'Done',
    failed: 'Failed',
  },
  common: {
    loading: 'Loading…',
  },
};

export default enUS;
