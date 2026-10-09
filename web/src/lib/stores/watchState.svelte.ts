let version = $state(0);

export function watchStateVersion(): number {
  return version;
}

export function bumpWatchStateVersion(): void {
  version++;
}
