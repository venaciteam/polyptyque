const colors = { normal: '#7c8cff', active: '#3dd68c', recording: '#ff4d5a', warning: '#f5b83d', offline: '#8a92a0' };
export const escape = text => String(text ?? '').replace(/[&<>"']/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&apos;' })[c]);
function width(text) { return Array.from(text).reduce((n, c) => n + (/[^\u0000-\u024f]/u.test(c) ? 1.05 : /[MW@]/.test(c) ? 0.85 : /[il .,:]/.test(c) ? 0.3 : 0.56), 0); }
export function fit(text, size = 24, max = 126) {
  const chars = Array.from(String(text ?? ''));
  if (width(chars.join('')) * 12 > max) {
    while (chars.length && width(`${chars.join('')}…`) * 12 > max) chars.pop();
    chars.push('…');
  }
  const value = chars.join('');
  return { text: escape(value), size: Math.max(12, Math.min(size, max / Math.max(1, width(value)))) };
}
export function keySvg({ title, value, detail = '', tone = 'normal' }) {
  const c = colors[tone] || colors.normal;
  const heading = fit(title, 16), main = fit(value, 28), sub = fit(detail, 13);
  return `<svg xmlns="http://www.w3.org/2000/svg" width="144" height="144" viewBox="0 0 144 144">
    <defs><clipPath id="text"><rect x="9" y="36" width="126" height="94"/></clipPath></defs>
    <rect width="144" height="144" rx="22" fill="#0e1014"/>
    <rect x="3" y="3" width="138" height="138" rx="20" fill="#16191f" stroke="${c}" stroke-width="2"/>
    <g fill="${c}"><rect x="53" y="12" width="10" height="22" rx="2" opacity=".48"/><rect x="67" y="12" width="10" height="22" rx="2"/><rect x="81" y="12" width="10" height="22" rx="2" opacity=".48"/></g>
    <g font-family="Segoe UI,Arial,sans-serif" text-anchor="middle" clip-path="url(#text)">
      <text x="72" y="56" fill="#e8eaee" font-size="${heading.size}">${heading.text}</text>
      <text x="72" y="90" fill="${c}" font-weight="600" font-size="${main.size}">${main.text}</text>
      <text x="72" y="118" fill="#8a92a0" font-size="${sub.size}">${sub.text}</text>
    </g></svg>`;
}
export const dataImage = value => `data:image/svg+xml;charset=utf8,${encodeURIComponent(keySvg(value))}`;
