import { describe, expect, it } from 'vitest';
import { ghostKey, ghostSuffix } from './ghostText';

function field(value: string, caret = value.length) {
  return { value, selectionStart: caret, selectionEnd: caret };
}

function key(
  name: string,
  mods: Partial<Record<'shiftKey' | 'altKey' | 'ctrlKey' | 'metaKey', boolean>> = {}
) {
  return { key: name, shiftKey: false, altKey: false, ctrlKey: false, metaKey: false, ...mods };
}

describe('ghostSuffix', () => {
  it.each([
    ['dun', 'Dune', 'e'],
    ['DUNE m', 'Dune Messiah', 'essiah'],
    ['dune', 'Dune', ''],
    ['dux', 'Dune', ''],
    ['', 'Dune', ''],
    ['   ', 'Dune', ''],
    ['dun', undefined, '']
  ] as const)('%j against %j shows %j', (typed, suggestion, ghost) => {
    expect(ghostSuffix(field(typed), suggestion)).toBe(ghost);
  });

  it('shows nothing unless the caret sits collapsed at the end', () => {
    expect(ghostSuffix(field('dun', 1), 'Dune')).toBe('');
    expect(ghostSuffix({ value: 'dun', selectionStart: 0, selectionEnd: 3 }, 'Dune')).toBe('');
  });
});

describe('ghostKey', () => {
  it('accepts with Tab or ArrowRight at the end of the text', () => {
    expect(ghostKey(key('Tab'), field('dun'), 'Dune')).toBe('Dune');
    expect(ghostKey(key('ArrowRight'), field('dun'), 'Dune')).toBe('Dune');
  });

  it('leaves ArrowRight mid-text to move the caret', () => {
    expect(ghostKey(key('ArrowRight'), field('dun', 1), 'Dune')).toBeNull();
  });

  it('leaves Tab to move focus when there is no ghost', () => {
    expect(ghostKey(key('Tab'), field('dux'), 'Dune')).toBeNull();
    expect(ghostKey(key('Tab'), field('dune'), 'Dune')).toBeNull();
    expect(ghostKey(key('Tab'), field('dun'), undefined)).toBeNull();
  });

  it('ignores Shift+Tab, modified arrows and other keys', () => {
    expect(ghostKey(key('Tab', { shiftKey: true }), field('dun'), 'Dune')).toBeNull();
    expect(ghostKey(key('ArrowRight', { ctrlKey: true }), field('dun'), 'Dune')).toBeNull();
    expect(ghostKey(key('Enter'), field('dun'), 'Dune')).toBeNull();
  });
});
