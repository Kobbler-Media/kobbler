/**
 * Settings field highlight utility.
 *
 * When navigating to a settings page with #field=xxx in the URL hash,
 * scrolls to and highlights the matching field with a brief glow animation.
 */

const HIGHLIGHT_CLASS = 'field-highlight';
const HIGHLIGHT_DURATION_MS = 2500;

function highlightField(): void {
  const hash = window.location.hash;
  if (!hash.startsWith('#field=')) return;

  const fieldName = hash.slice('#field='.length);
  if (!fieldName) return;

  // Small delay to ensure DOM is rendered after navigation
  requestAnimationFrame(() => {
    const el = document.querySelector(`[data-field="${CSS.escape(fieldName)}"]`);
    if (!el) return;

    el.scrollIntoView({ behavior: 'smooth', block: 'center' });
    el.classList.add(HIGHLIGHT_CLASS);

    setTimeout(() => {
      el.classList.remove(HIGHLIGHT_CLASS);
      // Clean hash without triggering navigation
      history.replaceState(null, '', window.location.pathname + window.location.search);
    }, HIGHLIGHT_DURATION_MS);
  });
}

/**
 * Initialize settings field highlighting.
 * Call from settings layout onMount. Returns a cleanup function.
 */
export function initSettingsHighlight(): () => void {
  highlightField();

  const onHashChange = () => highlightField();
  window.addEventListener('hashchange', onHashChange);

  return () => {
    window.removeEventListener('hashchange', onHashChange);
  };
}
