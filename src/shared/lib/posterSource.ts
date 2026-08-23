import { convertFileSrc } from '@tauri-apps/api/core';

export type PosterSize = 'w92' | 'w342';

export function posterCacheName(posterPath: string, size: PosterSize): string {
  const name = posterPath.replace(/^\//, '');
  return size === 'w92' ? `w92_${name}` : name;
}

export function posterSource(
  posterPath: string,
  size: PosterSize,
  revision: number,
): string {
  const source = convertFileSrc(posterCacheName(posterPath, size), 'poster');
  return `${source}?v=${revision}`;
}
