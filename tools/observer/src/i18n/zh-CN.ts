const zhCN = {
  app: {
    wordmark: 'AWR',
    title: '观测台',
  },
  nav: {
    workbench: '工作台',
    projects: '项目',
    inbox: '待处理',
    knowledge: '知识与技能',
    members: '成员与 Agent',
    activity: '活动与审计',
    settings: '管理设置',
    account: '账户与连接',
    advanced: '高级工具',
    groupMain: '观测',
    groupAdmin: '管理',
  },
  shell: {
    scope: '示例组织',
    searchPlaceholder: '搜索项目、工作项…',
    notifications: '通知',
    language: '语言',
    role: '观察者',
  },
  project: {
    settings: '项目设置',
    metaKey: '项目键',
    metaRole: '角色',
    tabs: {
      overview: '概览',
      plan: '工作计划',
      deps: '协作依赖',
      acceptance: '成果与验收',
      knowledge: '项目知识',
    },
  },
  freshness: {
    title: '新鲜度',
    plane: {
      report_received: '上报接收',
      authoritative: '权威测量',
      page_read: '页面读取',
    },
    state: {
      measured: '已测量',
      stale: '已过期',
      failed: '读取失败',
      not_measured: '未测量',
    },
  },
  state: {
    empty: {
      title: '未观察到相关记录',
      description: '截至 {{time}} 的读取，未观察到相关记录。',
      noRead: '尚未完成读取，未观察到相关记录。',
    },
    forbidden: {
      subtitle: '没有查看该范围的权限，受限内容已清除。',
    },
    notReported: {
      title: '尚未收到上报',
      description: '该范围还没有收到任何进度上报；这不等同于没有进展。',
    },
    partial: {
      alert: '部分读取失败',
      description: '以下内容来自读取失败前已确认的部分；失败原因：{{code}}。',
      confirmedAt: '确认于 {{time}}',
    },
    placeholder: {
      title: '该页面为占位',
      description: '此观测视图将在后续切片交付；当前不展示任何业务数据。',
    },
  },
  coverage: {
    summary: '范围 {{scope}} · 成功 {{succeeded}} · 失败 {{failed}} · 工作项 {{enumerated}}/{{total}}',
    totalUnknown: '未知',
  },
  account: {
    title: '账户与连接',
    login: '登录',
    loginBearer: '访问令牌',
    loginSubmit: '登录',
    logout: '退出',
    logoutSubmit: '退出登录',
    connect: '连接我的 Agent',
    connectSubmit: '发起连接',
    revoke: '撤销连接',
    revokeSubmit: '撤销全部连接',
    done: '操作完成',
    failed: '操作失败',
  },
  common: {
    loading: '加载中…',
  },
};

export default zhCN;
