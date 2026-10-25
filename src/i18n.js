import messages from './locales.json' with { type: 'json' };

export const supportedLanguages = ['en', 'ko'];
export function normalizeLanguage(value) {
  return supportedLanguages.includes(value) ? value : 'en';
}

export function translate(language, key, values = {}) {
  const template = messages[key]?.[normalizeLanguage(language)] ?? messages[key]?.en;
  if (template === undefined) throw new Error(`Missing translation: ${key}`);
  return template.replace(/\{(\w+)\}/g, (match, name) => values[name] ?? match);
}

export function applyTranslations(root, language) {
  for (const attribute of ['text', 'placeholder', 'title', 'aria-label']) {
    const selector = attribute === 'text' ? 'data-i18n' : `data-i18n-${attribute}`;
    for (const element of root.querySelectorAll(`[${selector}]`)) {
      const value = translate(language, element.getAttribute(selector));
      if (attribute === 'text') element.textContent = value;
      else element.setAttribute(attribute, value);
    }
  }
}
