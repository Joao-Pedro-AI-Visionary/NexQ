// Binary search over ascending segment start times.

/**
 * Index of the last start that is <= timeMs, or -1 when timeMs is before the
 * first start (or there are no starts). `startsMs` must be sorted ascending.
 */
export function findActiveSegmentIndex(startsMs: number[], timeMs: number): number {
  let lo = 0;
  let hi = startsMs.length - 1;
  let found = -1;
  while (lo <= hi) {
    const mid = (lo + hi) >>> 1;
    if (startsMs[mid] <= timeMs) {
      found = mid;
      lo = mid + 1;
    } else {
      hi = mid - 1;
    }
  }
  return found;
}
