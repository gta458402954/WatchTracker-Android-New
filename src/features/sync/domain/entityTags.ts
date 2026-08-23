/** Pure WebDAV entity-tag normalization helpers. */

export type EntityTagKind = 'strong' | 'weak';

function hasInvalidOpaqueCharacters(value: string): boolean {
  return [...value].some(character => {
    const code = character.charCodeAt(0);
    return character === '"' || code < 32 || code === 127;
  });
}

/** Classifies an already quoted entity tag, preserving the HTTP strong/weak distinction. */
export function entityTagKind(value: string | null | undefined): EntityTagKind | null {
  if (!value) return null;
  const weak = value.startsWith('W/');
  const opaque = weak ? value.slice(2) : value;
  if (opaque.length < 3 || !opaque.startsWith('"') || !opaque.endsWith('"')) return null;
  if (hasInvalidOpaqueCharacters(opaque.slice(1, -1))) return null;
  return weak ? 'weak' : 'strong';
}

/** Normalizes a server value to a quoted HTTP entity tag when it is safe to use. */
export function normalizedEntityTag(value: string | null | undefined): string | null {
  const trimmed = value?.trim();
  if (!trimmed) return null;
  if (entityTagKind(trimmed)) return trimmed;
  const weak = trimmed.startsWith('W/');
  const opaque = weak ? trimmed.slice(2) : trimmed;
  if (!opaque || hasInvalidOpaqueCharacters(opaque)) return null;
  return `${weak ? 'W/' : ''}"${opaque}"`;
}

/** Selects the first safe ETag from values extracted by the transport's XML parser. */
export function firstUsableEntityTag(values: Iterable<string | null | undefined>): string | null {
  for (const value of values) {
    const normalized = normalizedEntityTag(value);
    if (normalized) return normalized;
  }
  return null;
}

export const normalizeEntityTag = normalizedEntityTag;
export const isStrongEntityTag = (value: string | null | undefined): value is string => entityTagKind(value) === 'strong';
export const isEntityTag = (value: string | null | undefined): value is string => entityTagKind(value) !== null;
export const strongEtag = isStrongEntityTag;
export const entityTag = isEntityTag;
