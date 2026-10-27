import type { StreamGroup, StreamOption } from '$lib/api/client';

export interface StreamSection {
  title: string | null;
  options: StreamOption[];
}

const TITLES: Record<StreamGroup, string> = {
  played: 'Played before',
  account: 'On your debrid account',
  available: 'Available',
  uncached: 'Needs downloading',
  failed: 'Previously failed'
};

/** Consecutive runs of one group, in kobload's order; kobload ranks by group first. */
export function streamSections(options: StreamOption[]): StreamSection[] {
  const sections: StreamSection[] = [];
  for (const option of options) {
    const title = option.group ? TITLES[option.group] : null;
    const last = sections.at(-1);
    if (last && last.title === title) last.options.push(option);
    else sections.push({ title, options: [option] });
  }
  return sections;
}
