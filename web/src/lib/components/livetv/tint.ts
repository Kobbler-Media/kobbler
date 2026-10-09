// The guide feed's Affiliate is the whole legal name — "ABC (Australian
// Broadcasting Corporation)", "Network Ten (Australia)" — so an equality check
// against a bare network key matched nothing and every channel fell through to
// the hash. Order matters only in that each needle must be absent from the
// others' names, which holds for these five.
const NETWORK_TINTS: [string, string][] = [
  ['abc', '#6dc3d6'],
  ['sbs', '#a98ce5'],
  ['seven', '#5fcf8a'],
  ['nine', '#82a4e8'],
  ['ten', '#e3c36c']
];

const PALETTE = ['#6dc3d6', '#a98ce5', '#5fcf8a', '#82a4e8', '#e3c36c', '#e08a8a', '#8fd0a8'];

const NEUTRAL = '#4a525c';

function paletteTint(key: string): string {
  let hash = 0;
  for (let i = 0; i < key.length; i++) hash = (hash * 31 + key.charCodeAt(i)) >>> 0;
  return PALETTE[hash % PALETTE.length];
}

function normalize(value: string | null | undefined): string | null {
  const key = (value ?? '').trim().toLowerCase();
  if (!key || key === '—' || key === 'unidentified') return null;
  return key;
}

export function networkTint(network: string | null | undefined): string {
  const key = normalize(network);
  if (!key) return NEUTRAL;
  const known = NETWORK_TINTS.find(([needle]) => key.includes(needle));
  return known ? known[1] : paletteTint(key);
}

// Genres get the palette and nothing else: they share no vocabulary with the
// network names, and a substring match against those would colour a genre by
// coincidence.
export function genreTint(genre: string | null | undefined): string {
  const key = normalize(genre);
  return key ? paletteTint(key) : NEUTRAL;
}

export function typeTint(typeLabel: string | null | undefined): string {
  const key = (typeLabel ?? '').toLowerCase();
  if (key.startsWith('hd')) return '#5fcf8a';
  if (key.startsWith('radio')) return '#a98ce5';
  if (key.startsWith('dead') || key.startsWith('unnamed')) return '#5f6874';
  return '#9da6b0';
}

export function signalTint(quality: number | null | undefined): string {
  if (quality == null) return 'var(--ring)';
  if (quality >= 95) return 'var(--success)';
  if (quality >= 85) return 'var(--warning)';
  return 'var(--danger)';
}
