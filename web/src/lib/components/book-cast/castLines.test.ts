import { describe, expect, it } from 'vitest';
import { firstPassName, namesLabel } from './castLines.js';

describe('firstPassName', () => {
  const pattern = { name: 'Pattern' };
  it.each([
    ['a name other than the canonical one', 'the spren', pattern, 'the spren'],
    ['the canonical name in another case', 'pattern', pattern, null],
    ['no first-pass name', null, pattern, null],
    ['no character', 'the spren', undefined, null]
  ])('%s', (_, speaker, character, want) => {
    expect(firstPassName({ speaker }, character)).toBe(want);
  });
});

describe('namesLabel', () => {
  it('lists each name with its lines, zero included', () => {
    expect(
      namesLabel([
        { name: 'Pattern', lines: 445 },
        { name: 'the spren', lines: 26 },
        { name: 'Stormfather', lines: 0 }
      ])
    ).toBe('Pattern 445 · the spren 26 · Stormfather 0');
  });
});
