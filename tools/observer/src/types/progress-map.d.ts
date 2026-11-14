declare module 'awr-progress-map/project-map/snapshot.js' {
  const snapshot: {
    SCHEMA: string;
    VERSION: number;
    STATUSES: string[];
    canonical(value: unknown): string;
    sha256Hex(value: string): string;
    seal(content: Record<string, unknown>, generatedAt: string): Record<string, unknown> & { fingerprint: string };
  };
  export default snapshot;
}

declare module 'awr-progress-map/project-map/page.js' {
  const page: {
    makeT(catalogs: Record<string, Record<string, string>>, language: string): (key: string, params?: Record<string, unknown>) => string;
    renderProjectMap(snapshot: Record<string, unknown>, config: unknown, options: {
      t: (key: string, params?: Record<string, unknown>) => string;
      lang: string;
      interactive: boolean;
    }): { body: string; model: unknown };
  };
  export default page;
}

declare module 'awr-progress-map/project-map/interact.js' {
  const interaction: {
    mount(options: {
      container: HTMLElement;
      snapshot: Record<string, unknown>;
      config?: unknown;
      model: unknown;
      t: (key: string, params?: Record<string, unknown>) => string;
      restore?: unknown;
      commandFor?: (key: string) => string;
      technicalDetails?: boolean;
    }): { state: unknown; destroy(): void };
  };
  export default interaction;
}

declare module 'awr-progress-map/locales/en.js' {
  const messages: Record<string, string>;
  export default messages;
}

declare module 'awr-progress-map/locales/zh-CN.js' {
  const messages: Record<string, string>;
  export default messages;
}
