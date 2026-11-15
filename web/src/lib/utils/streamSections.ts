import type { StreamGroup, StreamOption } from '#lib/api/client.js';

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

/**
 * Consecutive runs of one group, in kobload's order; kobload ranks by group first.
 * Reported releases are pulled out to the end: one reported here keeps its
 * place in the list until the next search.
 */
export function streamSections(options: StreamOption[]): StreamSection[] {
  const sections: StreamSection[] = [];
  const reported: StreamOption[] = [];
  for (const option of options) {
    if (option.reported_bad) {
      reported.push(option);
      continue;
    }
    const title = option.group ? TITLES[option.group] : null;
    const last = sections.at(-1);
    if (last && last.title === title) last.options.push(option);
    else sections.push({ title, options: [option] });
  }
  if (reported.length > 0) sections.push({ title: 'Reported bad', options: reported });
  return sections;
}
