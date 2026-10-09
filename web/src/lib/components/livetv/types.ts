export type FacetGroupId = 'network' | 'region' | 'type' | 'guide' | 'status';

export type FacetPill = {
  value: string;
  label: string;
  count?: number;
  tint?: string;
};

export type FacetToggle = {
  value: string;
  label: string;
  active: boolean;
};

export type TunerLease = {
  id: string;
  kind: 'watching' | 'recording';
  label: string;
};
