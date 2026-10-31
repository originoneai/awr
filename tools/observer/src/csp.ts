/**
 * CSP integration shim (declared hand-written piece): antd's cssinjs honours
 * ConfigProvider's csp.nonce, but some rc modules inject their own <style>
 * elements without one — e.g. @rc-component/util's measureScrollbarSize,
 * which calls dynamicCSS.updateCSS(css, randomId) with no csp option, so a
 * strict `style-src 'self' 'nonce-…'` blocks it and logs a console error.
 * The injected CSS contains a random element id, so a CSP hash cannot work,
 * and 'unsafe-inline' is not acceptable. Authorizing every <style> element
 * this app creates with the page's own per-response nonce (at creation,
 * before insertion) is exactly what the nonce is for and covers every
 * library path, present and future.
 */
export function installStyleNonceShim(): void {
  if (typeof document === 'undefined') return;
  const original = document.createElement.bind(document);
  document.createElement = ((tagName: string, options?: ElementCreationOptions) => {
    const element = original(tagName, options);
    if (tagName.toLowerCase() === 'style' && window.__CSP_NONCE__) {
      element.nonce = window.__CSP_NONCE__;
    }
    return element;
  }) as typeof document.createElement;
}
