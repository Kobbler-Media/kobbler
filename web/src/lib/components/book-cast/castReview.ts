import type { BookCastCharacter, BookCastVoice } from '#lib/types.js';

export interface VoiceStep {
  key: string;
  character: BookCastCharacter;
  voice: BookCastVoice | null;
  sharedWith: BookCastCharacter[];
  approved: boolean;
}

function rank(c: BookCastCharacter): number {
  if (c.narrator) return 0;
  return c.pooled ? 2 : 1;
}

/**
 * Mirrors the server's `unapproved`: approval is per preset, so a pool sharing
 * one preset is a single step however many speakers read with it.
 */
export function voiceSteps(characters: BookCastCharacter[]): VoiceStep[] {
  const ordered = characters
    .filter((c) => c.narrator || !c.generic)
    .toSorted((a, b) => rank(a) - rank(b) || b.line_count - a.line_count);
  const steps: VoiceStep[] = [];
  const pools = new Map<string, VoiceStep>();
  for (const c of ordered) {
    if (c.voices.length === 0) {
      steps.push({ key: c.id, character: c, voice: null, sharedWith: [], approved: false });
      continue;
    }
    for (const v of c.voices) {
      const pool = c.pooled && !c.narrator && v.preset_id ? pools.get(v.preset_id) : undefined;
      if (pool) {
        pool.sharedWith.push(c);
        continue;
      }
      const step = {
        key: `${c.id}:${v.from_chapter}`,
        character: c,
        voice: v,
        sharedWith: [],
        approved: v.approved_at !== null && v.preset_id !== null
      };
      if (c.pooled && !c.narrator && v.preset_id) pools.set(v.preset_id, step);
      steps.push(step);
    }
  }
  return steps;
}

export function firstOpen(steps: { approved: boolean }[], from: number): number {
  const n = steps.length;
  for (let i = 1; i <= n; i++) {
    const at = (from + i) % n;
    if (!steps[at].approved) return at;
  }
  return -1;
}

function words(s: string): Set<string> {
  return new Set(s.toLowerCase().match(/[\p{L}\p{N}']+/gu) ?? []);
}

function namesOf(c: BookCastCharacter): string[] {
  return [c.name, ...c.aliases];
}

function named(name: string, inText: Set<string>): boolean {
  const parts = [...words(name)];
  return parts.length > 0 && parts.every((w) => inText.has(w));
}

export function suggestSpeakers(
  characters: BookCastCharacter[],
  line: { speaker: string | null; text: string },
  limit = 6
): BookCastCharacter[] {
  const speaking = characters
    .filter((c) => !c.narrator)
    .toSorted((a, b) => b.line_count - a.line_count);
  const guess = line.speaker ? words(line.speaker) : new Set<string>();
  const inText = words(line.text);
  const out: BookCastCharacter[] = [];
  const add = (c: BookCastCharacter) => {
    if (!out.includes(c)) out.push(c);
  };
  for (const c of speaking) {
    if (guess.size > 0 && namesOf(c).some((n) => [...guess].every((w) => words(n).has(w)))) add(c);
  }
  for (const c of speaking) {
    if (namesOf(c).some((n) => named(n, inText))) add(c);
  }
  for (const c of speaking) add(c);
  return out.slice(0, limit);
}
