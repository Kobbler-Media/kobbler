import type { BookCastName } from '#lib/types.js';

export function firstPassName(
  line: { speaker: string | null },
  character: { name: string } | undefined
): string | null {
  if (!character || !line.speaker) return null;
  return line.speaker.toLowerCase() === character.name.toLowerCase() ? null : line.speaker;
}

export function namesLabel(names: BookCastName[]): string {
  return names.map((n) => `${n.name} ${n.lines}`).join(' · ');
}
