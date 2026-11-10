import type { BookCastCharacter } from '#lib/types.js';

export const BIBLE_FIELDS = ['gender', 'age', 'accent', 'personality'] as const;

export function bibleText(
  character: BookCastCharacter,
  field: (typeof BIBLE_FIELDS)[number]
): string | null {
  const v = character.bible[field];
  return typeof v === 'string' && v.trim() ? v : null;
}

export function partLabel(character: BookCastCharacter): string | null {
  if (character.narrator) return 'Narrator';
  if (character.generic) return 'Read by the narrator';
  if (character.pooled) return 'Minor part';
  return null;
}

export function speakerHue(id: string): number {
  let h = 0;
  for (let i = 0; i < id.length; i++) h = (h * 31 + id.charCodeAt(i)) >>> 0;
  return h % 360;
}
