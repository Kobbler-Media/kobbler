import { describe, expect, it } from 'vitest';
import { firstPassName, paragraphs } from './castLines.js';

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

describe('paragraphs', () => {
  const para = '“Hi,” said Ann. “Bye,” said Tom.';
  const line = (segment_index: number, quote: string, text = para) => {
    const start = text.indexOf(quote);
    return {
      segment_index,
      id: quote,
      context: {
        before: null,
        pre: text.slice(0, start),
        quote,
        post: text.slice(start + quote.length),
        after: null
      }
    };
  };

  it('stitches every quote in a paragraph between the narration around it', () => {
    const [p] = paragraphs([line(3, '“Hi,”'), line(3, '“Bye,”')]);
    expect(p.segment_index).toBe(3);
    expect(p.pieces.map((x) => (x.line ? `[${x.line.id}]` : x.text))).toEqual([
      '[“Hi,”]',
      ' said Ann. ',
      '[“Bye,”]',
      ' said Tom.'
    ]);
  });

  it('keeps each paragraph apart, in order', () => {
    const got = paragraphs([line(1, '“Hi,”'), line(2, '“Bye,”')]);
    expect(got.map((p) => p.segment_index)).toEqual([1, 2]);
  });

  it('counts offsets in the characters the browser sees', () => {
    const text = 'Café — “Oui,” said Zoë.';
    const [p] = paragraphs([line(0, '“Oui,”', text)]);
    expect(p.pieces.map((x) => x.text)).toEqual(['Café — ', '“Oui,”', ' said Zoë.']);
  });

  it('shows a line with no context as the quote alone', () => {
    const [p] = paragraphs([{ segment_index: 0, id: 'x', context: null, text: 'Hello' }]);
    expect(p.pieces.map((x) => x.text)).toEqual(['“Hello”']);
  });
});
