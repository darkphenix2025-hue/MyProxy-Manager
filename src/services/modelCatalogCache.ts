import type { CodexModel } from '../types/codex';

export interface CachedModel {
  readonly id: string;
  readonly displayName?: string;
  readonly ownedBy?: string;
}

export interface CachedModelCatalog {
  readonly models: ReadonlyArray<CachedModel>;
  readonly updatedAt: number;
}

type ModelCatalogStore = Record<string, CachedModelCatalog>;

const STORAGE_KEY = 'myproxy_model_catalog_cache_v1';
export const MODEL_CATALOG_UPDATED_EVENT = 'myproxy:model-catalog-updated';

export function providerModelCatalogKey(providerId: string): string {
  return `provider:${providerId}`;
}

export function codexModelCatalogKey(credentialId: string): string {
  return `codex:${credentialId}`;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null;
}

function normalizeModel(value: unknown): CachedModel | null {
  if (typeof value === 'string') {
    const id = value.trim();
    return id ? { id } : null;
  }
  if (!isRecord(value) || typeof value.id !== 'string' || !value.id.trim()) {
    return null;
  }
  const model: CachedModel = { id: value.id.trim() };
  const displayName = typeof value.displayName === 'string' ? value.displayName.trim() : '';
  const ownedBy = typeof value.ownedBy === 'string' ? value.ownedBy.trim() : '';
  return {
    ...model,
    ...(displayName ? { displayName } : {}),
    ...(ownedBy ? { ownedBy } : {}),
  };
}

function normalizeCatalog(value: unknown): CachedModelCatalog | null {
  if (!isRecord(value) || !Array.isArray(value.models)) {
    return null;
  }
  const models = dedupeModels(value.models.map(normalizeModel).filter((model): model is CachedModel => model !== null));
  const updatedAt = typeof value.updatedAt === 'number' && Number.isFinite(value.updatedAt)
    ? value.updatedAt
    : 0;
  return { models, updatedAt };
}

function readStore(): ModelCatalogStore {
  if (typeof localStorage === 'undefined') return {};
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return {};
    const parsed: unknown = JSON.parse(raw);
    if (!isRecord(parsed)) return {};
    return Object.fromEntries(
      Object.entries(parsed)
        .map(([key, value]) => [key, normalizeCatalog(value)] as const)
        .filter((entry): entry is readonly [string, CachedModelCatalog] => entry[1] !== null),
    );
  } catch {
    return {};
  }
}

function writeStore(store: ModelCatalogStore): void {
  if (typeof localStorage === 'undefined') return;
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(store));
    window.dispatchEvent(new CustomEvent(MODEL_CATALOG_UPDATED_EVENT));
  } catch {
    // Model discovery must remain usable even when local storage is unavailable.
  }
}

export function dedupeModels(models: ReadonlyArray<CachedModel>): CachedModel[] {
  const byId = new Map<string, CachedModel>();
  for (const model of models) {
    const id = model.id.trim();
    if (!id) continue;
    const existing = byId.get(id);
    byId.set(id, {
      id,
      ...(model.displayName || existing?.displayName
        ? { displayName: model.displayName || existing?.displayName }
        : {}),
      ...(model.ownedBy || existing?.ownedBy
        ? { ownedBy: model.ownedBy || existing?.ownedBy }
        : {}),
    });
  }
  return Array.from(byId.values()).sort((left, right) => left.id.localeCompare(right.id));
}

export function getModelCatalog(key: string): CachedModelCatalog | null {
  return readStore()[key] ?? null;
}

export function setModelCatalog(key: string, models: ReadonlyArray<CachedModel | string | CodexModel>): CachedModelCatalog {
  const normalized = models
    .map((model) => {
      if (typeof model === 'string') return { id: model };
      const displayName = 'display_name' in model && typeof model.display_name === 'string'
        ? model.display_name
        : 'displayName' in model && typeof model.displayName === 'string'
          ? model.displayName
          : undefined;
      const ownedBy = 'owned_by' in model && typeof model.owned_by === 'string'
        ? model.owned_by
        : 'ownedBy' in model && typeof model.ownedBy === 'string'
          ? model.ownedBy
          : undefined;
      return {
        id: model.id,
        ...(displayName ? { displayName } : {}),
        ...(ownedBy ? { ownedBy } : {}),
      };
    })
    .map(normalizeModel)
    .filter((model): model is CachedModel => model !== null);
  const catalog: CachedModelCatalog = {
    models: dedupeModels(normalized),
    updatedAt: Date.now(),
  };
  const store = readStore();
  store[key] = catalog;
  writeStore(store);
  return catalog;
}

export function removeModelCatalog(key: string): void {
  const store = readStore();
  if (!(key in store)) return;
  delete store[key];
  writeStore(store);
}

export function getAllModelCatalogs(): ModelCatalogStore {
  return readStore();
}
