// Global format filter state — persisted to localStorage, applies across all library pages
export type FormatFilter = 'all' | 'ebook' | 'audiobook';

let current: FormatFilter = $state('all');

// Initialize from localStorage
if (typeof localStorage !== 'undefined') {
  const saved = localStorage.getItem('kobbler-format-filter');
  if (saved === 'ebook' || saved === 'audiobook') {
    current = saved;
  }
}

export function getFormatFilter(): FormatFilter {
  return current;
}

export function setFormatFilter(value: FormatFilter) {
  const prev = current;
  current = value;
  if (typeof localStorage !== 'undefined') {
    if (value === 'all') {
      localStorage.removeItem('kobbler-format-filter');
    } else {
      localStorage.setItem('kobbler-format-filter', value);
    }
  }
  // Notify pages to reload data with the new filter
  if (prev !== value && typeof window !== 'undefined') {
    window.dispatchEvent(new CustomEvent('formatfilterchange'));
  }
}
