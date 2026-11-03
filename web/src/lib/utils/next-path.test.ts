import { describe, expect, it } from 'vitest';
import { safeNextPath } from './next-path';

describe('safeNextPath', () => {
  it.each([
    ['/player/', '/player/'],
    ['/player/?x=1#y', '/player/?x=1#y'],
    [null, null],
    ['', null],
    ['player', null],
    ['//evil.example/', null],
    ['/\\evil.example', null],
    ['https://evil.example/', null],
    ['javascript:alert(1)', null]
  ])('%s -> %s', (input, expected) => {
    expect(safeNextPath(input)).toBe(expected);
  });
});
