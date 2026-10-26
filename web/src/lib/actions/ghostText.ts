type Field = Pick<HTMLInputElement, 'value' | 'selectionStart' | 'selectionEnd'>;
type Key = Pick<KeyboardEvent, 'key' | 'shiftKey' | 'altKey' | 'ctrlKey' | 'metaKey'>;

export function ghostSuffix(field: Field, suggestion: string | undefined): string {
  const typed = field.value;
  const end = typed.length;
  if (!suggestion || !typed.trim() || field.selectionStart !== end || field.selectionEnd !== end)
    return '';
  if (suggestion.length <= end || !suggestion.toLowerCase().startsWith(typed.toLowerCase()))
    return '';
  return suggestion.slice(end);
}

export function ghostKey(e: Key, field: Field, suggestion: string | undefined): string | null {
  if (e.shiftKey || e.altKey || e.ctrlKey || e.metaKey) return null;
  if (e.key !== 'Tab' && e.key !== 'ArrowRight') return null;
  return ghostSuffix(field, suggestion) ? (suggestion ?? null) : null;
}

export type GhostTextParams = {
  suggestion: string | undefined;
  onaccept: (value: string) => void;
};

const MIRRORED = [
  'fontFamily',
  'fontSize',
  'fontWeight',
  'fontStyle',
  'letterSpacing',
  'textTransform',
  'paddingLeft',
  'paddingRight',
  'paddingTop',
  'paddingBottom',
  'borderLeftWidth',
  'borderRightWidth',
  'borderTopWidth',
  'borderBottomWidth',
  'boxSizing'
] as const;

export function ghostText(input: HTMLInputElement, initial: GhostTextParams) {
  let params = initial;
  const overlay = document.createElement('div');
  const typed = document.createElement('span');
  const ghost = document.createElement('span');
  overlay.setAttribute('aria-hidden', 'true');
  Object.assign(overlay.style, {
    position: 'absolute',
    pointerEvents: 'none',
    whiteSpace: 'pre',
    overflow: 'hidden',
    display: 'flex',
    alignItems: 'center',
    borderStyle: 'solid',
    borderColor: 'transparent'
  });
  typed.style.visibility = 'hidden';
  ghost.style.color = 'var(--muted)';
  overlay.append(typed, ghost);
  input.after(overlay);
  const parent = input.parentElement;
  if (parent && getComputedStyle(parent).position === 'static') parent.style.position = 'relative';

  function refresh() {
    const suffix = document.activeElement === input ? ghostSuffix(input, params.suggestion) : '';
    overlay.hidden = !suffix;
    if (!suffix) return;
    const style = getComputedStyle(input);
    for (const prop of MIRRORED) overlay.style[prop] = style[prop];
    Object.assign(overlay.style, {
      left: `${input.offsetLeft}px`,
      top: `${input.offsetTop}px`,
      width: `${input.offsetWidth}px`,
      height: `${input.offsetHeight}px`
    });
    typed.textContent = input.value;
    ghost.textContent = suffix;
    overlay.scrollLeft = input.scrollLeft;
  }

  function keydown(e: KeyboardEvent) {
    const accepted = ghostKey(e, input, params.suggestion);
    if (accepted === null) return;
    e.preventDefault();
    e.stopPropagation();
    params.onaccept(accepted);
    queueMicrotask(refresh);
  }

  const events = ['input', 'keyup', 'click', 'select', 'focus', 'blur'] as const;
  for (const name of events) input.addEventListener(name, refresh);
  input.addEventListener('keydown', keydown);
  refresh();

  return {
    update(next: GhostTextParams) {
      params = next;
      refresh();
    },
    destroy() {
      for (const name of events) input.removeEventListener(name, refresh);
      input.removeEventListener('keydown', keydown);
      overlay.remove();
    }
  };
}
