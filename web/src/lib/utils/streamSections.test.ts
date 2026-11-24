import { describe, expect, it } from 'vitest';
import type { StreamGroup, StreamOption } from '#lib/api/client.js';
import { streamSections } from './streamSections';

const option = (id: string, group?: StreamGroup, reported_bad?: boolean) =>
  ({ id, group, reported_bad }) as StreamOption;

describe('streamSections', () => {
  it('runs the server order under one heading per group', () => {
    const sections = streamSections([
      option('a', 'played'),
      option('d', 'available'),
      option('e', 'uncached'),
      option('f', 'failed')
    ]);
    expect(sections.map((s) => [s.title, s.options.map((o) => o.id)])).toEqual([
      ['Played before', ['a']],
      ['Available', ['d']],
      ['Needs downloading', ['e']],
      ['Previously failed', ['f']]
    ]);
  });

  it('gathers reported releases under one last heading wherever they sit', () => {
    const sections = streamSections([
      option('a', 'available'),
      option('b', 'available', true),
      option('c', 'available'),
      option('d', 'failed'),
      option('e', 'failed', true)
    ]);
    expect(sections.map((s) => [s.title, s.options.map((o) => o.id)])).toEqual([
      ['Available', ['a', 'c']],
      ['Previously failed', ['d']],
      ['Reported bad', ['b', 'e']]
    ]);
  });

  it('lists the retired account group from an older server without its heading', () => {
    const sections = streamSections([
      option('a', 'played'),
      option('b', 'account' as StreamGroup),
      option('c', 'available')
    ]);
    expect(sections.map((s) => [s.title, s.options.map((o) => o.id)])).toEqual([
      ['Played before', ['a']],
      [null, ['b']],
      ['Available', ['c']]
    ]);
  });

  it('keeps ungrouped options from an older kobload as one untitled list', () => {
    const sections = streamSections([option('a'), option('b')]);
    expect(sections.map((s) => [s.title, s.options.map((o) => o.id)])).toEqual([
      [null, ['a', 'b']]
    ]);
  });
});
