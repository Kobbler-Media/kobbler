import { describe, expect, it } from 'vitest';
import type { BookCastCharacter, BookCastVoice } from '#lib/types.js';
import { firstOpen, suggestSpeakers, voiceSteps } from './castReview.js';

function voice(over: Partial<BookCastVoice> = {}): BookCastVoice {
  return {
    character_id: '',
    from_chapter: 0,
    preset_id: 'p',
    design_preset_id: 'p',
    approved_at: null,
    preset_name: null,
    has_clip: false,
    description: null,
    ...over
  };
}

function character(name: string, over: Partial<BookCastCharacter> = {}): BookCastCharacter {
  return {
    id: name,
    name,
    aliases: [],
    names: [],
    bible: {},
    line_count: 10,
    pooled: false,
    generic: false,
    narrator: false,
    samples: [],
    voices: [voice({ character_id: name, preset_id: name, design_preset_id: name })],
    ...over
  };
}

describe('voiceSteps', () => {
  it('puts the narrator first, then speaking parts by lines, pools last', () => {
    const steps = voiceSteps([
      character('Tom', { line_count: 3 }),
      character('guard', { pooled: true, line_count: 1 }),
      character('Ann', { line_count: 40 }),
      character('Narrator', { narrator: true, line_count: 0 })
    ]);
    expect(steps.map((s) => s.character.name)).toEqual(['Narrator', 'Ann', 'Tom', 'guard']);
  });

  it('leaves out generic parts, which the narrator reads', () => {
    expect(voiceSteps([character('crowd', { generic: true })])).toEqual([]);
  });

  it('gives each voice change its own step', () => {
    const ann = character('Ann', {
      voices: [voice({ character_id: 'Ann' }), voice({ character_id: 'Ann', from_chapter: 4 })]
    });
    expect(voiceSteps([ann]).map((s) => s.voice?.from_chapter)).toEqual([0, 4]);
  });

  it('reviews a pooled voice once for everyone sharing it', () => {
    const shared = (name: string) =>
      character(name, {
        pooled: true,
        voices: [voice({ character_id: name, preset_id: 'old-man', design_preset_id: 'old-man' })]
      });
    const steps = voiceSteps([shared('guard'), shared('innkeeper'), shared('priest')]);
    expect(steps).toHaveLength(1);
    expect(steps[0].character.name).toBe('guard');
    expect(steps[0].sharedWith.map((c) => c.name)).toEqual(['innkeeper', 'priest']);
  });

  it('marks approved voices, and a part with no voice as not approved', () => {
    const steps = voiceSteps([
      character('Ann', {
        voices: [voice({ character_id: 'Ann', approved_at: '2026-11-10T00:00:00Z' })]
      }),
      character('Tom', { voices: [] })
    ]);
    expect(steps.map((s) => [s.character.name, s.approved, s.voice === null])).toEqual([
      ['Ann', true, false],
      ['Tom', false, true]
    ]);
  });

  it('keys each step uniquely', () => {
    const steps = voiceSteps([
      character('Ann', {
        voices: [voice({ character_id: 'Ann' }), voice({ character_id: 'Ann', from_chapter: 2 })]
      }),
      character('Tom', { voices: [] })
    ]);
    expect(new Set(steps.map((s) => s.key)).size).toBe(steps.length);
  });
});

describe('firstOpen', () => {
  const steps = [{ approved: true }, { approved: false }, { approved: true }, { approved: false }];
  it.each([
    ['the first open step from the start', 0, 1],
    ['the next open step after the current one', 1, 3],
    ['wraps round to an earlier open step', 3, 1]
  ])('%s', (_, from, want) => {
    expect(firstOpen(steps, from)).toBe(want);
  });

  it('stays on the current step when it is the only one open', () => {
    expect(firstOpen([{ approved: true }, { approved: false }], 1)).toBe(1);
  });

  it('is -1 when everything is approved', () => {
    expect(firstOpen([{ approved: true }], 0)).toBe(-1);
  });
});

describe('suggestSpeakers', () => {
  const cast = [
    character('Narrator', { narrator: true }),
    character('Ann Smith', { aliases: ['Lizzy'], line_count: 50 }),
    character('Tom', { line_count: 5 }),
    character('Sadeas', { line_count: 20 }),
    character('crowd', { generic: true, line_count: 2 })
  ];

  it('suggests the first-pass guess first, then whoever the text names', () => {
    const got = suggestSpeakers(
      cast,
      {
        speaker: 'tom',
        text: '“Come,” said Lizzy, and Sadeas nodded.'
      },
      3
    );
    expect(got.map((c) => c.name)).toEqual(['Tom', 'Ann Smith', 'Sadeas']);
  });

  it('matches whole words only', () => {
    const got = suggestSpeakers(cast, { speaker: null, text: 'Tomorrow, then.' }, 3);
    expect(got.map((c) => c.name)).toEqual(['Ann Smith', 'Sadeas', 'Tom']);
  });

  it('falls back to the biggest parts, never the narrator', () => {
    const got = suggestSpeakers(cast, { speaker: null, text: 'Nobody named here.' }, 2);
    expect(got.map((c) => c.name)).toEqual(['Ann Smith', 'Sadeas']);
  });

  it('matches a first-pass name against a part of the character name', () => {
    const got = suggestSpeakers(cast, { speaker: 'Smith', text: '' }, 1);
    expect(got.map((c) => c.name)).toEqual(['Ann Smith']);
  });
});
