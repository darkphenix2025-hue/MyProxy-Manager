import type {
    ProviderModelConfig,
    ProviderProtocolConfig,
    UpstreamProvider,
} from '../types/config';

export function splitModelIds(value?: string): string[] {
    return Array.from(new Set(
        (value || '')
            .split(/[\n,]/)
            .map((model) => model.trim())
            .filter(Boolean),
    ));
}

export function modelConfigsForProvider(provider: UpstreamProvider): ProviderModelConfig[] {
    const byId = new Map<string, ProviderModelConfig>();
    for (const model of provider.model_configs || []) {
        const id = model.id?.trim();
        if (!id) continue;
        byId.set(id, { ...model, id });
    }
    for (const id of splitModelIds(provider.available_models)) {
        if (!byId.has(id)) byId.set(id, { id });
    }
    return Array.from(byId.values());
}

export function isProtocolSuffixName(name: string): boolean {
    return /(?:[_\-\s]?op)$/i.test(name.trim());
}

export function logicalProviderName(name: string): string {
    const normalized = name.trim().replace(/(?:[_\-\s]?op)$/i, '').trim();
    return normalized || name.trim();
}

export function legacyProtocolConfig(provider: UpstreamProvider): ProviderProtocolConfig {
    return {
        protocol: provider.protocol,
        enabled: provider.enabled,
        base_url: provider.base_url || '',
        api_key: provider.api_key || '',
        ...(provider.credential_id ? { credential_id: provider.credential_id } : {}),
        dispatch_mode: provider.dispatch_mode,
        priority: provider.priority,
        model_prefixes: [...(provider.model_prefixes || [])],
        model_mapping: { ...(provider.model_mapping || {}) },
        ...(provider.request_timeout_secs !== undefined
            ? { request_timeout_secs: provider.request_timeout_secs }
            : {}),
    };
}

export function protocolConfigsForProvider(provider: UpstreamProvider): ProviderProtocolConfig[] {
    if (provider.protocols && provider.protocols.length > 0) {
        return provider.protocols.map((config) => ({
            ...legacyProtocolConfig(provider),
            ...config,
            protocol: config.protocol,
            model_prefixes: [...(config.model_prefixes || [])],
            model_mapping: { ...(config.model_mapping || {}) },
        }));
    }
    return [legacyProtocolConfig(provider)];
}

export interface ProviderGroup {
    key: string;
    provider: UpstreamProvider;
    members: UpstreamProvider[];
}

function providerGroupKey(provider: UpstreamProvider): string {
    const explicitGroup = provider.provider_group?.trim();
    return explicitGroup
        ? `group:${explicitGroup.toLowerCase()}`
        : `name:${logicalProviderName(provider.name).toLowerCase()}`;
}

function mergeProviderGroup(key: string, members: UpstreamProvider[]): ProviderGroup {
    const base = members.find((provider) => !isProtocolSuffixName(provider.name)) || members[0];
    const protocolConfigs: ProviderProtocolConfig[] = [];
    for (const member of members) {
        for (const config of protocolConfigsForProvider(member)) {
            const existing = protocolConfigs.find((item) => item.protocol === config.protocol);
            if (existing) {
                existing.enabled = existing.enabled || config.enabled;
            } else {
                protocolConfigs.push({ ...config });
            }
        }
    }

    const enabledProtocol = protocolConfigs.find((config) => config.enabled)
        || protocolConfigs[0]
        || legacyProtocolConfig(base);
    const modelConfigs = modelConfigsForProvider({
        ...base,
        model_configs: members.flatMap((member) => member.model_configs || []),
        available_models: members
            .flatMap((member) => splitModelIds(member.available_models))
            .join(', '),
    });
    const providerIds = members.flatMap((member) => [
        ...(member.provider_id ? [member.provider_id] : []),
        ...(member.provider_id_aliases || []),
    ]);
    const primaryProviderId = base.provider_id || providerIds[0];
    const aliases = Array.from(new Set(providerIds.filter((id) => id && id !== primaryProviderId)));
    const provider: UpstreamProvider = {
        ...base,
        name: logicalProviderName(base.name),
        provider_id: primaryProviderId || undefined,
        provider_id_aliases: aliases,
        provider_group: key.startsWith('group:')
            ? key.slice('group:'.length)
            : logicalProviderName(base.name).toLowerCase(),
        enabled: members.some((member) => member.enabled),
        protocol: enabledProtocol.protocol,
        base_url: enabledProtocol.base_url,
        api_key: enabledProtocol.api_key,
        ...(enabledProtocol.credential_id
            ? { credential_id: enabledProtocol.credential_id }
            : { credential_id: undefined }),
        dispatch_mode: enabledProtocol.dispatch_mode,
        priority: enabledProtocol.priority,
        model_prefixes: [...enabledProtocol.model_prefixes],
        model_mapping: { ...enabledProtocol.model_mapping },
        request_timeout_secs: enabledProtocol.request_timeout_secs,
        protocols: protocolConfigs,
        model_configs: modelConfigs,
        available_models: modelConfigs.map((model) => model.id).filter(Boolean).join(', '),
    };
    return { key, provider, members };
}

export function groupProviders(rawProviders: UpstreamProvider[]): ProviderGroup[] {
    const buckets = new Map<string, UpstreamProvider[]>();
    for (const provider of rawProviders) {
        const key = providerGroupKey(provider);
        const bucket = buckets.get(key) || [];
        bucket.push(provider);
        buckets.set(key, bucket);
    }
    return Array.from(buckets.entries()).map(([key, members]) => mergeProviderGroup(key, members));
}
