import { useState, useEffect, useMemo } from 'react';
import { useTranslation } from 'react-i18next';
import { request as invoke } from '../utils/request';
import {
    BrainCircuit,
    Sparkles,
    Plus,
    Trash2,
    Edit2,
    Check,
    X,
    RefreshCw,
    Save,
    ShieldAlert,
    Activity,
    Clock,
    Zap,
    ChevronDown,
    ChevronUp,
} from 'lucide-react';
import { ModelCooldownEntry, ExperimentalConfig, StickySessionConfig, WeightedTarget } from '../types/config';
import HelpTooltip from '../components/common/HelpTooltip';
import ModalDialog from '../components/common/ModalDialog';
import { showToast } from '../components/common/ToastContainer';
import GroupedSelect, { type SelectOption } from '../components/common/GroupedSelect';
import ProviderModelSelect from '../components/common/ProviderModelSelect';
import ReasoningEffortSelect from '../components/common/ReasoningEffortSelect';
import DebouncedSlider from '../components/common/DebouncedSlider';
import CircuitBreaker from '../components/settings/CircuitBreaker';
import { useProxyConfig } from '../hooks/useProxyConfig';
import { listAccounts } from '../services/accountService';
import {
    codexModelCatalogKey,
    getModelCatalog,
    MODEL_CATALOG_UPDATED_EVENT,
    providerModelCatalogKey,
} from '../services/modelCatalogCache';
import type { CodexConnection } from '../types/codex';
import { groupProviders, protocolConfigsForProvider } from '../utils/providerGrouping';
import type {
    ModelSelectionOption,
    ModelSelectionProtocol,
    ModelSelectionSource,
} from '../components/common/ProviderModelSelect';
import { getProviderProtocolLabel } from '../components/common/ProviderModelSelect';

interface CustomPreset {
    id: string;
    name: string;
    description: string;
    mappings: Record<string, string | WeightedTarget[]>;
}

const weightedSourceModelTemplates: ReadonlyArray<SelectOption> = [
    { value: 'internal-background-task', label: '后台任务 · internal-background-task', group: '系统模板' },
    { value: 'claude-opus-*', label: 'Claude Opus · claude-opus-*', group: 'Claude' },
    { value: 'claude-sonnet-*', label: 'Claude Sonnet · claude-sonnet-*', group: 'Claude' },
    { value: 'claude-haiku-*', label: 'Claude Haiku · claude-haiku-*', group: 'Claude' },
    { value: 'claude-*', label: 'Claude 全系列 · claude-*', group: 'Claude' },
    { value: 'gpt-5.4*', label: 'GPT-5.4 · gpt-5.4*', group: 'GPT' },
    { value: 'gpt-5.3*', label: 'GPT-5.3 · gpt-5.3*', group: 'GPT' },
    { value: 'gpt-5.2*', label: 'GPT-5.2 · gpt-5.2*', group: 'GPT' },
    { value: 'gpt-5.1*', label: 'GPT-5.1 · gpt-5.1*', group: 'GPT' },
    { value: 'gpt-5*', label: 'GPT-5 全系列 · gpt-5*', group: 'GPT' },
    { value: 'gpt-4.1*', label: 'GPT-4.1 · gpt-4.1*', group: 'GPT' },
    { value: 'gpt-4o*', label: 'GPT-4o · gpt-4o*', group: 'GPT' },
    { value: 'gpt-4*', label: 'GPT-4 全系列 · gpt-4*', group: 'GPT' },
    { value: 'gpt-*', label: 'GPT 全系列 · gpt-*', group: 'GPT' },
    { value: 'gemini-3*', label: 'Gemini 3 · gemini-3*', group: 'Gemini' },
    { value: 'gemini-2.5-pro*', label: 'Gemini 2.5 Pro · gemini-2.5-pro*', group: 'Gemini' },
    { value: 'gemini-2.5-flash*', label: 'Gemini 2.5 Flash · gemini-2.5-flash*', group: 'Gemini' },
    { value: 'gemini-2*', label: 'Gemini 2 全系列 · gemini-2*', group: 'Gemini' },
    { value: 'gemini-*', label: 'Gemini 全系列 · gemini-*', group: 'Gemini' },
];

const DEFAULT_MAPPING_WEIGHT = 1;
const MAX_MAPPING_WEIGHT = 1_000_000;
const TARGET_EFFORT_SEPARATOR = '::';

const targetEffortKey = (routePattern: string, target: string): string =>
    `${routePattern}${TARGET_EFFORT_SEPARATOR}${target}`;

const normalizeMappingWeight = (weight: number): number => Number.isFinite(weight)
    ? Math.min(MAX_MAPPING_WEIGHT, Math.max(1, Math.round(weight)))
    : DEFAULT_MAPPING_WEIGHT;

const formatMappingPercentage = (weight: number, totalWeight: number): string => {
    if (totalWeight <= 0) return '0%';
    const percentage = Number(((weight / totalWeight) * 100).toFixed(2));
    return `${percentage}%`;
};

/** Convert both legacy single-target mappings and weighted mappings to rows. */
const normalizeMappingTargets = (value?: string | WeightedTarget[]): WeightedTarget[] => {
    if (!value) return [];

    if (typeof value === 'string') {
        const target = value.trim();
        return target ? [{ target, weight: DEFAULT_MAPPING_WEIGHT }] : [];
    }

    return value
        .map((entry) => ({
            target: entry.target.trim(),
            weight: normalizeMappingWeight(entry.weight),
        }))
        .filter((entry) => entry.target.length > 0);
};

/** Merge targets by model ID so repeated single-target additions build 1:N mappings. */
const mergeMappingTargets = (
    current: ReadonlyArray<WeightedTarget>,
    incoming: ReadonlyArray<WeightedTarget>,
): WeightedTarget[] => {
    const merged = current.map((entry) => ({ ...entry }));
    for (const entry of incoming) {
        const target = entry.target.trim();
        if (!target) continue;
        const existingIndex = merged.findIndex((candidate) => candidate.target === target);
        if (existingIndex >= 0) {
            merged[existingIndex] = { target, weight: entry.weight };
        } else {
            merged.push({ target, weight: entry.weight });
        }
    }
    return merged;
};

export default function RouteManager() {
    const { t } = useTranslation();
    const { appConfig, configLoading, configError, status, setAppConfig, saveConfig } = useProxyConfig();

    // Model routing state
    const [editingTarget, setEditingTarget] = useState<{ key: string; index: number } | null>(null);
    const [editingTargetValue, setEditingTargetValue] = useState<string>('');
    const [mappingDrafts, setMappingDrafts] = useState<Record<string, WeightedTarget[]>>({});
    const [selectedPreset, setSelectedPreset] = useState<string>('default');
    const [customPresets, setCustomPresets] = useState<CustomPreset[]>([]);
    const [isPresetManagerOpen, setIsPresetManagerOpen] = useState(false);
    const [newPresetName, setNewPresetName] = useState('');
    const [isResetConfirmOpen, setIsResetConfirmOpen] = useState(false);
    const [preferredAccountId, setPreferredAccountId] = useState<string | null>(null);
    const [availableAccounts, setAvailableAccounts] = useState<Array<{ id: string; email: string }>>([]);
    const [codexConnections, setCodexConnections] = useState<ReadonlyArray<CodexConnection>>([]);
    const [modelCatalogVersion, setModelCatalogVersion] = useState(0);
    const [isClearBindingsConfirmOpen, setIsClearBindingsConfirmOpen] = useState(false);

    // Weighted mapping state
    const [weightedKey, setWeightedKey] = useState('');
    const [weightedTargets, setWeightedTargets] = useState<WeightedTarget[]>([{ target: '', weight: DEFAULT_MAPPING_WEIGHT }]);
    const [weightedEffort, setWeightedEffort] = useState<string>('');

    // Fallback model state
    const [fallbackEnabled, setFallbackEnabled] = useState(appConfig?.proxy?.fallback_model?.enabled ?? false);
    const [fallbackModel, setFallbackModel] = useState(appConfig?.proxy?.fallback_model?.model ?? '');
    const [fallbackProviderId, setFallbackProviderId] = useState(appConfig?.proxy?.fallback_model?.provider_id ?? '');
    const [fallbackEffort, setFallbackEffort] = useState<string>(appConfig?.proxy?.fallback_model?.reasoning_effort ?? '');

    // Cooldown state
    const [cooldowns, setCooldowns] = useState<ModelCooldownEntry[]>([]);
    const [cooldownEnabled, setCooldownEnabled] = useState(appConfig?.proxy?.model_cooldown?.enabled ?? true);
    const [cooldownDuration, setCooldownDuration] = useState(appConfig?.proxy?.model_cooldown?.duration_secs ?? 600);

    const getMappingEffort = (key: string, target: string): string => {
        const routeEffort = appConfig?.proxy.route_reasoning_effort ?? {};
        return (target ? routeEffort[targetEffortKey(key, target)] : undefined)
            ?? routeEffort[key]
            ?? '';
    };

    const modelSources: ReadonlyArray<ModelSelectionSource> = useMemo(() => {
        const sources: ModelSelectionSource[] = [];
        const providerGroups = groupProviders(appConfig?.proxy?.providers ?? []);

        for (const group of providerGroups) {
            const provider = group.provider;
            const providerId = provider.provider_id?.trim();
            if (!provider.enabled || !providerId) continue;

            const catalog = getModelCatalog(providerModelCatalogKey(providerId));
            const models = new Map<string, ModelSelectionOption>();
            for (const modelId of (provider.available_models ?? '').split(',')) {
                const id = modelId.trim();
                if (id) models.set(id, { value: id, label: id, group: '可用模型' });
            }
            for (const model of provider.model_configs ?? []) {
                const id = model.id.trim();
                if (!id) continue;
                const displayName = model.alias?.trim() || model.display_name?.trim();
                models.set(id, {
                    value: id,
                    label: displayName && displayName !== id ? `${displayName} (${id})` : id,
                    group: '可用模型',
                    ...(model.reasoning_efforts?.length
                        ? { reasoningEfforts: model.reasoning_efforts }
                        : {}),
                });
            }
            for (const model of catalog?.models ?? []) {
                if (models.has(model.id)) continue;
                models.set(model.id, {
                    value: model.id,
                    label: model.displayName && model.displayName !== model.id
                        ? `${model.displayName} (${model.id})`
                        : model.id,
                    group: '可用模型',
                });
            }

            const protocolRoutePrefixes = new Map<string, Set<string>>();
            for (const member of group.members) {
                const memberId = member.provider_id?.trim();
                if (!memberId) continue;
                for (const protocolConfig of protocolConfigsForProvider(member)) {
                    if (!protocolConfig.enabled) continue;
                    const prefixes = protocolRoutePrefixes.get(protocolConfig.protocol) ?? new Set<string>();
                    prefixes.add(memberId);
                    protocolRoutePrefixes.set(protocolConfig.protocol, prefixes);
                }
            }

            const configuredProtocols = protocolConfigsForProvider(provider);
            const enabledProtocols = configuredProtocols.filter((protocol) => protocol.enabled);
            const protocolConfigs = enabledProtocols.length > 0 ? enabledProtocols : configuredProtocols;
            const protocols: ModelSelectionProtocol[] = protocolConfigs.map((protocolConfig) => ({
                key: `provider:${providerId}:protocol:${protocolConfig.protocol}`,
                label: getProviderProtocolLabel(protocolConfig.protocol),
                protocol: protocolConfig.protocol,
                targetPrefix: providerId,
                targetPrefixes: Array.from(protocolRoutePrefixes.get(protocolConfig.protocol) ?? [])
                    .filter((prefix) => prefix !== providerId),
                models: Array.from(models.values()),
            }));

            sources.push({
                key: `provider:${providerId}`,
                label: provider.name || providerId,
                kind: 'provider',
                protocol: protocols[0]?.protocol ?? provider.protocol,
                targetPrefix: providerId,
                targetPrefixes: provider.provider_id_aliases,
                models: Array.from(models.values()),
                protocols,
            });
        }

        for (const connection of codexConnections) {
            if (!connection.connection.enabled) continue;
            const credentialId = connection.credential.id;
            const catalog = getModelCatalog(codexModelCatalogKey(credentialId));
            const identity = connection.identity.email
                || connection.identity.display_name
                || connection.credential.fingerprint;
            const protocol: ModelSelectionProtocol = {
                key: `codex:${credentialId}:protocol:codex_responses`,
                label: getProviderProtocolLabel('codex_responses'),
                protocol: 'codex_responses',
                targetPrefix: `account/${credentialId}`,
                models: (catalog?.models ?? []).map((model) => ({
                    value: model.id,
                    label: model.displayName && model.displayName !== model.id
                        ? `${model.displayName} (${model.id})`
                        : model.id,
                    group: '可用模型',
                })),
            };
            sources.push({
                key: `codex:${credentialId}`,
                label: identity,
                kind: 'account',
                protocol: 'codex_responses',
                targetPrefix: `account/${credentialId}`,
                models: protocol.models,
                protocols: [protocol],
            });
        }

        return sources;
    }, [appConfig?.proxy?.providers, codexConnections, modelCatalogVersion]);

    const customMappingEntries = useMemo(() => {
        const mappings = appConfig?.proxy?.custom_mapping ?? {};
        return Object.entries(mappings)
            .map(([key, value]) => ({
                key,
                targets: mappingDrafts[key] ?? normalizeMappingTargets(value),
            }))
            .filter((entry) => entry.targets.length > 0);
    }, [appConfig?.proxy?.custom_mapping, mappingDrafts]);

    // Load custom presets from localStorage
    useEffect(() => {
        try {
            const storageKey = 'myproxy_custom_presets';
            const legacyStorageKey = 'antigravity_custom_presets';
            const saved = localStorage.getItem(storageKey) ?? localStorage.getItem(legacyStorageKey);
            if (saved) {
                setCustomPresets(JSON.parse(saved));
                localStorage.setItem(storageKey, saved);
                localStorage.removeItem(legacyStorageKey);
            }
        } catch (error) {
            console.error('Failed to load custom presets:', error);
        }
        loadAccounts();
        loadCodexConnections();
        loadPreferredAccount();

        const handleModelCatalogUpdated = () => setModelCatalogVersion((version) => version + 1);
        window.addEventListener(MODEL_CATALOG_UPDATED_EVENT, handleModelCatalogUpdated);
        return () => window.removeEventListener(MODEL_CATALOG_UPDATED_EVENT, handleModelCatalogUpdated);
    }, []);

    // Load cooldowns on mount
    useEffect(() => {
        loadCooldowns();
        const refreshTimer = window.setInterval(loadCooldowns, 5000);
        return () => window.clearInterval(refreshTimer);
    }, []);

    // Keep the countdown responsive between backend refreshes. The backend
    // remains authoritative and removes expired entries on the next fetch.
    useEffect(() => {
        const timer = window.setInterval(() => {
            setCooldowns((current) => current
                .map((cooldown) => ({
                    ...cooldown,
                    remaining_secs: Math.max(0, cooldown.remaining_secs - 1),
                }))
                .filter((cooldown) => cooldown.remaining_secs > 0));
        }, 1000);
        return () => window.clearInterval(timer);
    }, []);

    // Sync fallback/cooldown state from config
    useEffect(() => {
        if (appConfig?.proxy?.fallback_model) {
            setFallbackEnabled(appConfig.proxy.fallback_model.enabled);
            setFallbackModel(appConfig.proxy.fallback_model.model);
            setFallbackProviderId(appConfig.proxy.fallback_model.provider_id ?? '');
            setFallbackEffort(appConfig.proxy.fallback_model.reasoning_effort ?? '');
        }
    }, [appConfig?.proxy?.fallback_model]);

    // Sync cooldown state from config
    useEffect(() => {
        if (appConfig?.proxy?.model_cooldown) {
            setCooldownEnabled(appConfig.proxy.model_cooldown.enabled);
            setCooldownDuration(appConfig.proxy.model_cooldown.duration_secs ?? 600);
        }
    }, [appConfig?.proxy?.model_cooldown]);

    // Default presets
    const defaultPresets = useMemo(() => [
        {
            id: 'default',
            name: t('proxy.router.preset_default'),
            description: t('proxy.router.preset_default_desc'),
            mappings: {
                "claude-sonnet-*": "big/glm-5-turbo",
                "claude-opus-*": "big/glm-5.1",
                "claude-haiku-*": "big/glm-5-turbo",
                "gpt-5.4*": "big/glm-5.1",
                "gpt-5.3*": "big/glm-5-turbo",
                "gpt-5.2*": "big/glm-5-turbo",
                "gpt-5.1*": "big/glm-5-turbo"
            }
        },
        {
            id: 'performance',
            name: t('proxy.router.preset_performance'),
            description: t('proxy.router.preset_performance_desc'),
            mappings: {
                "claude-sonnet-*": "ali/qwen3.5-plus",
                "claude-opus-*": "ali/qwen3.6-plus",
                "claude-haiku-*": "ali/qwen3.5-plus",
                "gpt-5.4*": "ali/qwen3.6-plus",
                "gpt-5.3*": "ali/qwen3.5-plus",
                "gpt-5.2*": "ali/qwen3.5-plus",
                "gpt-5.1*": "ali/qwen3.5-plus"
            }
        },
        {
            id: 'cost-effective',
            name: t('proxy.router.preset_cost'),
            description: t('proxy.router.preset_cost_desc'),
            mappings: {
                "claude-sonnet-*": "ali/qwen3.5-plus",
                "claude-opus-*": "ali/qwen3.6-plus",
                "claude-haiku-*": "ali/kimi-k2.5",
                "gpt-5.4*": "ali/qwen3.6-plus",
                "gpt-5.3*": "ali/qwen3.5-plus",
                "gpt-5.2*": "ali/qwen3.5-plus",
                "gpt-5.1*": "ali/kimi-k2.5"
            }
        },
        {
            id: 'balanced',
            name: t('proxy.router.preset_balanced'),
            description: t('proxy.router.preset_balanced_desc'),
            mappings: {
                "claude-sonnet-*": "big/glm-5-turbo",
                "claude-opus-*": "big/glm-5.1",
                "claude-haiku-*": "big/glm-5-turbo",
                "gpt-5.4*": "big/glm-5.1",
                "gpt-5.3*": "big/glm-5-turbo",
                "gpt-5.2*": "big/glm-5-turbo",
                "gpt-5.1*": "big/glm-5-turbo"
            }
        },
    ], [t]);

    const presetOptions = useMemo(() => {
        return [...defaultPresets, ...customPresets];
    }, [defaultPresets, customPresets]);

    // Preset management
    const saveCustomPresetsToStorage = (presets: CustomPreset[]) => {
        try {
            localStorage.setItem('myproxy_custom_presets', JSON.stringify(presets));
            setCustomPresets(presets);
        } catch (error) {
            console.error('Failed to save custom presets:', error);
            showToast('Failed to save preset', 'error');
        }
    };

    const handleSaveCurrentAsPreset = () => {
        if (!appConfig?.proxy.custom_mapping || Object.keys(appConfig.proxy.custom_mapping).length === 0) {
            showToast(t('proxy.router.no_mapping_to_save'), 'warning');
            return;
        }
        if (!newPresetName.trim()) {
            showToast(t('proxy.router.preset_name_required'), 'warning');
            return;
        }

        const newPreset: CustomPreset = {
            id: `custom_${Date.now()}`,
            name: newPresetName,
            description: t('proxy.router.custom_preset_desc', { defaultValue: 'Custom preset' }),
            mappings: { ...appConfig.proxy.custom_mapping }
        };

        const updatedPresets = [...customPresets, newPreset];
        saveCustomPresetsToStorage(updatedPresets);
        setNewPresetName('');
        setIsPresetManagerOpen(false);
        showToast(t('proxy.router.preset_saved', { defaultValue: 'Preset saved successfully' }), 'success');
        setSelectedPreset(newPreset.id);
    };

    const handleDeletePreset = (id: string) => {
        const updatedPresets = customPresets.filter(p => p.id !== id);
        saveCustomPresetsToStorage(updatedPresets);
        if (selectedPreset === id) {
            setSelectedPreset('default');
        }
    };

    const handleApplyPresets = async () => {
        if (!appConfig) return;
        const selectedPresetData = presetOptions.find(p => p.id === selectedPreset);
        if (!selectedPresetData) return;

        const newConfig = {
            ...appConfig.proxy,
            custom_mapping: { ...appConfig.proxy.custom_mapping, ...selectedPresetData.mappings }
        };
        const oldConfig = { ...appConfig };

        try {
            setAppConfig({ ...appConfig, proxy: newConfig });
            setMappingDrafts({});
            showToast(t('proxy.router.presets_applied') + ` (${selectedPresetData.name})`, 'success');
            await saveConfig({ ...appConfig, proxy: newConfig });
        } catch (error) {
            console.error('Failed to apply presets:', error);
            setAppConfig(oldConfig);
            showToast(`${t('common.error')}: ${error}`, 'error');
        }
    };

    const persistMappingTargets = async (
        key: string,
        targets: ReadonlyArray<WeightedTarget>,
    ): Promise<boolean> => {
        if (!appConfig) return false;

        const normalizedTargets = normalizeMappingTargets([...targets]);
        const customMapping = { ...(appConfig.proxy.custom_mapping || {}) };
        const routeEffort = { ...(appConfig.proxy.route_reasoning_effort || {}) };
        const activeEffortKeys = new Set(
            normalizedTargets.map((target) => targetEffortKey(key, target.target)),
        );
        const effortPrefix = `${key}${TARGET_EFFORT_SEPARATOR}`;

        for (const effortKey of Object.keys(routeEffort)) {
            if (effortKey.startsWith(effortPrefix) && !activeEffortKeys.has(effortKey)) {
                delete routeEffort[effortKey];
            }
        }

        if (normalizedTargets.length === 0) {
            delete customMapping[key];
            delete routeEffort[key];
        } else {
            // Store rows consistently so a single target can later be edited
            // or joined with another target without changing the UI flow.
            customMapping[key] = normalizedTargets;
            const legacyEffort = routeEffort[key];
            if (legacyEffort) {
                for (const target of normalizedTargets) {
                    const effortKey = targetEffortKey(key, target.target);
                    if (!routeEffort[effortKey]) routeEffort[effortKey] = legacyEffort;
                }
                delete routeEffort[key];
            }
        }

        const newProxyConfig = {
            ...appConfig.proxy,
            custom_mapping: customMapping,
            route_reasoning_effort: routeEffort,
        };
        const newAppConfig = { ...appConfig, proxy: newProxyConfig };

        try {
            await saveConfig(newAppConfig);
            setAppConfig(newAppConfig);
            setMappingDrafts((current) => {
                const next = { ...current };
                delete next[key];
                return next;
            });
            return true;
        } catch (error) {
            // saveConfig already displays the single user-facing error toast.
            console.error(`Failed to save mapping '${key}':`, error);
            return false;
        }
    };

    const getMappingTargets = (key: string): WeightedTarget[] => {
        return mappingDrafts[key]
            ?? normalizeMappingTargets(appConfig?.proxy.custom_mapping?.[key]);
    };

    const handleRemoveCustomMapping = async (key: string) => {
        await persistMappingTargets(key, []);
        if (editingTarget?.key === key) {
            setEditingTarget(null);
            setEditingTargetValue('');
        }
    };

    const beginTargetEdit = (key: string, index: number, target: string) => {
        setEditingTarget({ key, index });
        setEditingTargetValue(target);
    };

    const cancelTargetEdit = () => {
        setEditingTarget(null);
        setEditingTargetValue('');
    };

    const saveTargetEdit = async () => {
        if (!editingTarget || !editingTargetValue.trim()) return;
        const targets = getMappingTargets(editingTarget.key);
        if (!targets[editingTarget.index]) return;

        const updatedTargets = targets.map((target, index) => index === editingTarget.index
            ? { ...target, target: editingTargetValue.trim() }
            : target);
        if (await persistMappingTargets(editingTarget.key, updatedTargets)) {
            cancelTargetEdit();
        }
    };

    const handleMappingWeightDraftChange = (key: string, index: number, weight: number) => {
        const targets = getMappingTargets(key);
        if (!targets[index]) return;
        const nextWeight = normalizeMappingWeight(weight);
        const updatedTargets = targets.map((target, targetIndex) => targetIndex === index
            ? { ...target, weight: nextWeight }
            : target);
        setMappingDrafts((current) => ({ ...current, [key]: updatedTargets }));
    };

    const commitMappingWeight = async (key: string) => {
        const draft = mappingDrafts[key];
        if (draft) await persistMappingTargets(key, draft);
    };

    const moveMappingTarget = async (key: string, index: number, direction: -1 | 1) => {
        const targets = getMappingTargets(key);
        const nextIndex = index + direction;
        if (nextIndex < 0 || nextIndex >= targets.length) return;

        const updatedTargets = [...targets];
        [updatedTargets[index], updatedTargets[nextIndex]] = [updatedTargets[nextIndex], updatedTargets[index]];
        if (editingTarget?.key === key) cancelTargetEdit();
        await persistMappingTargets(key, updatedTargets);
    };

    const removeMappingTarget = async (key: string, index: number) => {
        const targets = getMappingTargets(key);
        if (!targets[index]) return;

        const updatedTargets = targets.filter((_, targetIndex) => targetIndex !== index);
        if (editingTarget?.key === key) cancelTargetEdit();
        await persistMappingTargets(key, updatedTargets);
    };

    const handleMappingEffortUpdate = async (key: string, target: string, effort: string) => {
        if (!appConfig || !target) return;
        const routeEffort = { ...(appConfig.proxy.route_reasoning_effort || {}) };
        const legacyEffort = routeEffort[key];
        const targets = getMappingTargets(key);

        if (legacyEffort) {
            for (const mappingTarget of targets) {
                const effortKey = targetEffortKey(key, mappingTarget.target);
                if (!routeEffort[effortKey]) routeEffort[effortKey] = legacyEffort;
            }
            delete routeEffort[key];
        }

        const effortKey = targetEffortKey(key, target);
        if (effort) routeEffort[effortKey] = effort;
        else delete routeEffort[effortKey];

        const newProxyConfig = { ...appConfig.proxy, route_reasoning_effort: routeEffort };
        const newAppConfig = { ...appConfig, proxy: newProxyConfig };
        try {
            await saveConfig(newAppConfig);
            setAppConfig(newAppConfig);
        } catch (error) {
            console.error(`Failed to save reasoning effort for '${key}':`, error);
        }
    };

    // --- Weighted mapping handlers ---
    const handleAddWeightedMapping = async () => {
        const key = weightedKey.trim();
        const selectedTarget = weightedTargets[0];
        if (!appConfig || !key || !selectedTarget?.target.trim()) return;

        const incomingTarget: WeightedTarget = {
            target: selectedTarget.target.trim(),
            weight: normalizeMappingWeight(selectedTarget.weight),
        };

        const existingTargets = normalizeMappingTargets(appConfig.proxy.custom_mapping?.[key]);
        const mergedTargets = mergeMappingTargets(existingTargets, [incomingTarget]);
        const routeEffort = { ...(appConfig.proxy.route_reasoning_effort || {}) };
        const legacyEffort = routeEffort[key];
        if (legacyEffort) {
            for (const target of mergedTargets) {
                const effortKey = targetEffortKey(key, target.target);
                if (!routeEffort[effortKey]) routeEffort[effortKey] = legacyEffort;
            }
            delete routeEffort[key];
        }
        if (weightedEffort) {
            routeEffort[targetEffortKey(key, incomingTarget.target)] = weightedEffort;
        }
        const newProxyConfig = {
            ...appConfig.proxy,
            custom_mapping: { ...(appConfig.proxy.custom_mapping || {}), [key]: mergedTargets },
            route_reasoning_effort: routeEffort,
        };
        const newAppConfig = { ...appConfig, proxy: newProxyConfig };
        try {
            await saveConfig(newAppConfig);
            setAppConfig(newAppConfig);
            setMappingDrafts((current) => {
                const next = { ...current };
                delete next[key];
                return next;
            });
            setWeightedKey('');
            setWeightedTargets([{ target: '', weight: DEFAULT_MAPPING_WEIGHT }]);
            setWeightedEffort('');
            showToast(t('common.saved'), 'success');
        } catch (error) {
            // saveConfig already shows the single user-facing error toast.
            console.error('Failed to add weighted mapping:', error);
        }
    };

    const updateWeightedTarget = (index: number, field: keyof WeightedTarget, value: string | number) => {
        const updated = [...weightedTargets];
        const nextValue = field === 'weight' && typeof value === 'number'
            ? normalizeMappingWeight(value)
            : value;
        updated[index] = { ...updated[index], [field]: nextValue };
        setWeightedTargets(updated);
    };

    // --- Fallback model handlers ---
    const handleSaveFallbackModel = async () => {
        if (!appConfig) return;
        if (fallbackEnabled && !fallbackModel.trim()) {
            showToast('启用兜底模型时必须选择或输入模型', 'error');
            return;
        }
        const newProxyConfig = {
            ...appConfig.proxy,
            fallback_model: {
                enabled: fallbackEnabled,
                model: fallbackModel,
                provider_id: fallbackProviderId,
                reasoning_effort: fallbackEffort,
            }
        };
        try {
            await saveConfig({ ...appConfig, proxy: newProxyConfig });
            showToast(t('common.saved'), 'success');
        } catch (error) {
            showToast(`${t('common.error')}: ${error}`, 'error');
        }
    };

    // --- Cooldown handlers ---
    const loadCooldowns = async () => {
        try {
            const data = await invoke<ModelCooldownEntry[]>('get_model_cooldowns');
            setCooldowns(data);
        } catch { /* Service not running */ }
    };

    const handleClearCooldowns = async () => {
        try {
            await invoke('clear_model_cooldowns');
            setCooldowns([]);
            showToast('冷却状态已清除', 'success');
        } catch (error) {
            showToast(`${t('common.error')}: ${error}`, 'error');
        }
    };

    const handleSaveCooldownConfig = async () => {
        if (!appConfig) return;
        const newProxyConfig = {
            ...appConfig.proxy,
            model_cooldown: { enabled: cooldownEnabled, duration_secs: cooldownDuration }
        };
        try {
            await saveConfig({ ...appConfig, proxy: newProxyConfig });
            showToast(t('common.saved'), 'success');
        } catch (error) {
            showToast(`${t('common.error')}: ${error}`, 'error');
        }
    };

    const handleToggleCooldown = async (enabled: boolean) => {
        setCooldownEnabled(enabled);
        if (!appConfig) return;
        const newProxyConfig = {
            ...appConfig.proxy,
            model_cooldown: { enabled, duration_secs: cooldownDuration }
        };
        try {
            await saveConfig({ ...appConfig, proxy: newProxyConfig });
            showToast(t('common.saved'), 'success');
        } catch (error) {
            showToast(`${t('common.error')}: ${error}`, 'error');
        }
    };

    const handleResetMapping = () => {
        if (!appConfig) return;
        setIsResetConfirmOpen(true);
    };

    const executeResetMapping = async () => {
        if (!appConfig) return;
        setIsResetConfirmOpen(false);

        const newConfig = {
            ...appConfig.proxy,
            custom_mapping: {},
            route_reasoning_effort: {},
        };

        try {
            await saveConfig({ ...appConfig, proxy: newConfig });
            setAppConfig({ ...appConfig, proxy: newConfig });
            setMappingDrafts({});
            setEditingTarget(null);
            setEditingTargetValue('');
            showToast(t('common.success'), 'success');
        } catch (error) {
            console.error('Failed to reset mapping:', error);
            showToast(`${t('common.error')}: ${error}`, 'error');
        }
    };

    const updateExperimentalConfig = (updates: Partial<ExperimentalConfig>) => {
        if (!appConfig) return;
        const newConfig = {
            ...appConfig,
            proxy: {
                ...appConfig.proxy,
                experimental: {
                    ...(appConfig.proxy.experimental || {
                        enable_usage_scaling: true,
                        context_compression_threshold_l1: 0.4,
                        context_compression_threshold_l2: 0.55,
                        context_compression_threshold_l3: 0.7
                    }),
                    ...updates
                }
            }
        };
        saveConfig(newConfig);
    };

    const loadAccounts = async () => {
        try {
            const accounts = await listAccounts();
            setAvailableAccounts(accounts.map(a => ({ id: a.id, email: a.email })));
        } catch (error) {
            console.error('Failed to load accounts:', error);
        }
    };

    const loadCodexConnections = async () => {
        try {
            const connections = await invoke<CodexConnection[]>('list_account_connections');
            setCodexConnections(Array.isArray(connections) ? connections : []);
        } catch (error) {
            console.warn('Failed to load Codex auth-files for routing:', error);
        }
    };

    const loadPreferredAccount = async () => {
        try {
            const prefId = await invoke<string | null>('get_preferred_account');
            setPreferredAccountId(prefId);
        } catch (error) { /* Service not running, ignore */ }
    };

    const handleSetPreferredAccount = async (accountId: string | null) => {
        try {
            const wasEnabled = preferredAccountId !== null;
            await invoke('set_preferred_account', { accountId });
            setPreferredAccountId(accountId);
            let message: string;
            if (accountId === null) {
                message = t('proxy.config.scheduling.round_robin_set', { defaultValue: 'Round-robin mode enabled' });
            } else if (wasEnabled) {
                const account = availableAccounts.find(a => a.id === accountId);
                message = t('proxy.config.scheduling.account_changed', {
                    defaultValue: `Switched to ${account?.email || accountId}`,
                    email: account?.email || accountId
                });
            } else {
                message = t('proxy.config.scheduling.fixed_account_set', { defaultValue: 'Fixed account mode enabled' });
            }
            showToast(message, 'success');
        } catch (error) {
            showToast(String(error), 'error');
        }
    };

    const updateSchedulingConfig = (updates: Partial<StickySessionConfig>) => {
        if (!appConfig) return;
        const currentScheduling = appConfig.proxy.scheduling || { mode: 'Balance', max_wait_seconds: 60 };
        const newScheduling = { ...currentScheduling, ...updates };
        const newAppConfig = {
            ...appConfig,
            proxy: { ...appConfig.proxy, scheduling: newScheduling }
        };
        saveConfig(newAppConfig);
    };

    const handleClearSessionBindings = () => {
        setIsClearBindingsConfirmOpen(true);
    };

    const executeClearSessionBindings = async () => {
        setIsClearBindingsConfirmOpen(false);
        try {
            await invoke('clear_proxy_session_bindings');
            showToast(t('common.success'), 'success');
        } catch (error) {
            console.error('Failed to clear session bindings:', error);
            showToast(`${t('common.error')}: ${error}`, 'error');
        }
    };

    if (configLoading) {
        return (
            <div className="flex items-center justify-center min-h-[60vh]">
                <span className="loading loading-spinner loading-lg text-primary"></span>
            </div>
        );
    }

    if (configError || !appConfig) {
        return (
            <div className="alert alert-error w-full mx-auto mt-8">
                <span>{t('common.error')}: {configError || 'Configuration not loaded'}</span>
            </div>
        );
    }

    return (
        <div className="h-full w-full overflow-y-auto overflow-x-hidden">
            <div className="p-5 space-y-6 max-w-7xl mx-auto">
                {/* Model Routing Center - moved to top */}
                <div className="bg-white dark:bg-base-100 rounded-xl shadow-sm border border-gray-100 dark:border-base-200 overflow-hidden">
                    <div className="px-4 py-3 border-b border-gray-100 dark:border-gray-700/50 bg-gray-50/50 dark:bg-gray-800/50">
                        <div className="flex flex-col md:flex-row md:items-center justify-between gap-4">
                            <div className="flex-1">
                                <h2 className="text-base font-bold flex items-center gap-2 text-gray-900 dark:text-base-content">
                                    <BrainCircuit size={18} className="text-blue-500" />
                                    {t('proxy.router.title')}
                                </h2>
                                <p className="text-xs text-gray-500 dark:text-gray-400 mt-1 max-w-xl leading-relaxed">
                                    {t('proxy.router.subtitle_simple')}
                                </p>
                                <p className="mt-1 text-[10px] text-indigo-500 dark:text-indigo-300">
                                    模型列表按供应商/账号分组，使用本地缓存；请在对应页面点击“模型”或“从服务端获取”刷新
                                </p>
                            </div>
                            <div className="flex flex-wrap items-center gap-2 bg-white dark:bg-base-100 p-1.5 rounded-xl border border-gray-100 dark:border-gray-700/50 shadow-sm">
                                {/* Preset Select */}
                                <div className="relative min-w-[140px]">
                                    <select
                                        value={selectedPreset}
                                        onChange={(e) => setSelectedPreset(e.target.value)}
                                        className="select select-sm w-full bg-gray-50 dark:bg-base-200 border-gray-200 dark:border-gray-700 text-xs font-medium focus:ring-1 focus:ring-blue-500 h-9 min-h-0 rounded-lg"
                                    >
                                        <optgroup label={t('proxy.router.built_in_presets')}>
                                            {defaultPresets.map(preset => (
                                                <option key={preset.id} value={preset.id}>
                                                    {preset.name}
                                                </option>
                                            ))}
                                        </optgroup>
                                        {customPresets.length > 0 && (
                                            <optgroup label={t('proxy.router.custom_presets')}>
                                                {customPresets.map(preset => (
                                                    <option key={preset.id} value={preset.id}>
                                                        {preset.name}
                                                    </option>
                                                ))}
                                            </optgroup>
                                        )}
                                    </select>
                                </div>

                                <button
                                    onClick={handleApplyPresets}
                                    className="px-3 md:px-4 py-1.5 rounded-lg text-xs font-bold transition-all flex items-center gap-1.5 bg-blue-600 hover:bg-blue-700 text-white shadow-sm hover:shadow active:scale-95 h-9"
                                    title={presetOptions.find(p => p.id === selectedPreset)?.description}
                                >
                                    <Sparkles size={14} className="fill-white/20" />
                                    {t('proxy.router.apply_selected')}
                                </button>

                                <div className="w-[1px] h-5 bg-gray-200 dark:bg-gray-700 mx-1"></div>

                                {/* Add Preset */}
                                <button
                                    onClick={() => setIsPresetManagerOpen(true)}
                                    className="p-2 rounded-lg text-gray-500 hover:text-green-600 hover:bg-green-50 dark:hover:bg-green-900/20 transition-all h-9 w-9 flex items-center justify-center border border-transparent hover:border-green-100 dark:hover:border-green-900/30"
                                    title={t('proxy.router.add_preset')}
                                >
                                    <Plus size={16} />
                                </button>

                                {/* Delete Preset */}
                                <button
                                    onClick={() => {
                                        if (selectedPreset.startsWith('custom_')) {
                                            handleDeletePreset(selectedPreset);
                                        } else {
                                            showToast(t('proxy.router.cannot_delete_builtin', { defaultValue: 'Cannot delete built-in preset' }), 'warning');
                                        }
                                    }}
                                    className={`p-2 rounded-lg transition-all h-9 w-9 flex items-center justify-center border border-transparent ${selectedPreset.startsWith('custom_')
                                        ? 'text-gray-500 hover:text-red-600 hover:bg-red-50 dark:hover:bg-red-900/20 hover:border-red-100 dark:hover:border-red-900/30'
                                        : 'text-gray-300 dark:text-gray-600 cursor-not-allowed'
                                        }`}
                                    title={selectedPreset.startsWith('custom_')
                                        ? t('proxy.router.delete_preset')
                                        : t('proxy.router.cannot_delete_builtin', { defaultValue: 'Cannot delete built-in preset' })}
                                    disabled={!selectedPreset.startsWith('custom_')}
                                >
                                    <Trash2 size={16} />
                                </button>

                                <div className="w-[1px] h-5 bg-gray-200 dark:bg-gray-700 mx-1"></div>

                                <button
                                    onClick={handleResetMapping}
                                    className="p-2 rounded-lg text-gray-500 hover:text-red-600 hover:bg-red-50 dark:hover:bg-red-900/20 transition-all h-9 w-9 flex items-center justify-center border border-transparent hover:border-red-100 dark:hover:border-red-900/30"
                                    title={t('proxy.router.reset_mapping')}
                                >
                                    <RefreshCw size={16} />
                                </button>
                            </div>
                        </div>
                    </div>

                    <div className="p-3 space-y-3">
                        {/* Custom Mapping List */}
                        <div className="flex flex-col gap-4">
                            <div className="w-full flex flex-col">
                                <div className="mb-2 flex items-center justify-between">
                                    <span className="text-[10px] font-bold uppercase tracking-wider text-gray-500 dark:text-gray-400">
                                        {t('proxy.router.current_list')}
                                    </span>
                                    <span className="text-[10px] text-gray-400 dark:text-gray-500">
                                        点击目标模型可编辑
                                    </span>
                                </div>
                                <div
                                    className="max-h-[560px] overflow-y-auto rounded-lg border border-gray-200 bg-white p-3 dark:border-base-300 dark:bg-base-100"
                                    data-custom-mapping-list
                                >
                                    {customMappingEntries.length > 0 ? (
                                        <div className="space-y-3">
                                            {customMappingEntries.map(({ key, targets }) => {
                                                const totalWeight = targets.reduce((sum, target) => sum + target.weight, 0);
                                                return (
                                                    <section
                                                        key={key}
                                                        className="overflow-hidden rounded-lg border border-gray-200 bg-white dark:border-base-300 dark:bg-base-100"
                                                    >
                                                        <div className="flex flex-wrap items-center gap-2 border-b border-gray-100 px-3 py-2 dark:border-base-300">
                                                            <div className="min-w-0 flex-1">
                                                                <div className="flex flex-wrap items-center gap-2">
                                                                    <span className="max-w-full truncate font-mono text-[11px] font-bold text-blue-600 dark:text-blue-400" title={key}>
                                                                        {key}
                                                                    </span>
                                                                    <span className="rounded-full bg-blue-50 px-1.5 py-0.5 text-[9px] text-blue-600 dark:bg-blue-900/30 dark:text-blue-300">
                                                                        {targets.length} 个目标
                                                                    </span>
                                                                </div>
                                                                <div className="mt-0.5 text-[9px] text-gray-500 dark:text-gray-400">
                                                                    顺序用于优先级，模型权重按总比例计算实际占比
                                                                </div>
                                                            </div>
                                                            <div className="flex items-center gap-1.5">
                                                                <button
                                                                    type="button"
                                                                    onClick={() => handleRemoveCustomMapping(key)}
                                                                    className="btn btn-ghost btn-xs h-7 min-h-0 w-7 p-0 text-gray-400 hover:bg-red-50 hover:text-red-500 dark:hover:bg-red-900/20"
                                                                    title="删除整个模型模板"
                                                                    aria-label={`删除 ${key} 模型模板`}
                                                                >
                                                                    <Trash2 size={13} />
                                                                </button>
                                                            </div>
                                                        </div>

                                                        <div className="hidden grid-cols-[2rem_minmax(0,1fr)_8rem_7rem_7.5rem] items-center gap-2 px-3 py-1.5 text-[9px] text-gray-400 dark:text-gray-500 sm:grid">
                                                            <span className="text-center">优先级</span>
                                                            <span>目标模型</span>
                                                            <span className="text-center">思考深度</span>
                                                            <span className="text-center">模型权重 / 占比</span>
                                                            <span className="text-right">操作</span>
                                                        </div>
                                                        <div className="space-y-1.5 px-2 py-2">
                                                            {targets.map((target, index) => {
                                                                const isEditing = editingTarget?.key === key && editingTarget.index === index;
                                                                return (
                                                                    <div
                                                                        key={`${key}-${target.target}-${index}`}
                                                                        className={`rounded-md border px-2 py-1.5 transition-colors ${isEditing
                                                                            ? 'border-blue-300 bg-blue-50/70 dark:border-blue-700 dark:bg-blue-900/20'
                                                                            : 'border-gray-100 bg-white hover:border-blue-200 hover:bg-blue-50/30 dark:border-base-300 dark:bg-base-100 dark:hover:border-blue-800 dark:hover:bg-blue-900/10'}`}
                                                                    >
                                                                        <div className="grid grid-cols-[2rem_minmax(0,1fr)_8rem_7rem_7.5rem] items-center gap-2">
                                                                            <span className="text-center font-mono text-[10px] font-bold text-gray-500 dark:text-gray-400">
                                                                                {index + 1}
                                                                            </span>
                                                                            <div className="min-w-0">
                                                                                {isEditing ? (
                                                                                    <ProviderModelSelect
                                                                                        value={editingTargetValue}
                                                                                        onChange={setEditingTargetValue}
                                                                                        sources={modelSources}
                                                                                        placeholder="目标模型"
                                                                                        className="w-full min-w-0 font-mono text-[10px] lg:flex-nowrap"
                                                                                        allowCustomInput={true}
                                                                                    />
                                                                                ) : (
                                                                                    <button
                                                                                        type="button"
                                                                                        onClick={() => beginTargetEdit(key, index, target.target)}
                                                                                        className="flex w-full min-w-0 items-center gap-1 text-left"
                                                                                        title="点击编辑目标模型"
                                                                                    >
                                                                                        <span className="truncate font-mono text-[10px] text-gray-700 hover:text-blue-600 dark:text-gray-200 dark:hover:text-blue-300">
                                                                                            {target.target}
                                                                                        </span>
                                                                                        <Edit2 size={11} className="shrink-0 text-gray-400" />
                                                                                    </button>
                                                                                )}
                                                                            </div>
                                                                            <ReasoningEffortSelect
                                                                                value={getMappingEffort(key, target.target)}
                                                                                target={target.target}
                                                                                sources={modelSources}
                                                                                onChange={(effort) => handleMappingEffortUpdate(key, target.target, effort)}
                                                                                className="w-full min-w-0"
                                                                            />
                                                                            <div className="flex items-center justify-center gap-1">
                                                                                <input
                                                                                    type="number"
                                                                                    min={1}
                                                                                    max={MAX_MAPPING_WEIGHT}
                                                                                    step={1}
                                                                                    value={target.weight}
                                                                                    onChange={(event) => handleMappingWeightDraftChange(key, index, Number(event.target.value))}
                                                                                    onBlur={() => commitMappingWeight(key)}
                                                                                    className="input input-xs input-bordered h-7 w-12 bg-white text-center font-mono text-[10px] dark:bg-base-100"
                                                                                    title="模型比例权重（整数）"
                                                                                    aria-label={`${key} 第 ${index + 1} 个模型比例权重`}
                                                                                />
                                                                                <span className="min-w-[2.8rem] text-right font-mono text-[9px] text-blue-600 dark:text-blue-300">
                                                                                    {formatMappingPercentage(target.weight, totalWeight)}
                                                                                </span>
                                                                            </div>
                                                                            <div className="flex items-center justify-end gap-0.5">
                                                                                <button
                                                                                    type="button"
                                                                                    onClick={() => moveMappingTarget(key, index, -1)}
                                                                                    disabled={index === 0}
                                                                                    className="btn btn-ghost btn-xs h-6 min-h-0 w-6 p-0 text-gray-400 hover:bg-blue-50 hover:text-blue-500 disabled:opacity-30 dark:hover:bg-blue-900/20"
                                                                                    title="上移，提高优先级"
                                                                                    aria-label="上移"
                                                                                >
                                                                                    <ChevronUp size={13} />
                                                                                </button>
                                                                                <button
                                                                                    type="button"
                                                                                    onClick={() => moveMappingTarget(key, index, 1)}
                                                                                    disabled={index === targets.length - 1}
                                                                                    className="btn btn-ghost btn-xs h-6 min-h-0 w-6 p-0 text-gray-400 hover:bg-blue-50 hover:text-blue-500 disabled:opacity-30 dark:hover:bg-blue-900/20"
                                                                                    title="下移，降低优先级"
                                                                                    aria-label="下移"
                                                                                >
                                                                                    <ChevronDown size={13} />
                                                                                </button>
                                                                                {isEditing ? (
                                                                                    <>
                                                                                        <button
                                                                                            type="button"
                                                                                            onClick={saveTargetEdit}
                                                                                            disabled={!editingTargetValue.trim()}
                                                                                            className="btn btn-ghost btn-xs h-6 min-h-0 w-6 p-0 text-blue-500 hover:bg-blue-50 disabled:opacity-40 dark:hover:bg-blue-900/20"
                                                                                            title="保存目标模型"
                                                                                            aria-label="保存目标模型"
                                                                                        >
                                                                                            <Check size={13} strokeWidth={3} />
                                                                                        </button>
                                                                                        <button
                                                                                            type="button"
                                                                                            onClick={cancelTargetEdit}
                                                                                            className="btn btn-ghost btn-xs h-6 min-h-0 w-6 p-0 text-gray-400 hover:bg-gray-100 dark:hover:bg-base-300"
                                                                                            title="取消编辑"
                                                                                            aria-label="取消编辑"
                                                                                        >
                                                                                            <X size={13} />
                                                                                        </button>
                                                                                    </>
                                                                                ) : (
                                                                                    <button
                                                                                        type="button"
                                                                                        onClick={() => removeMappingTarget(key, index)}
                                                                                        className="btn btn-ghost btn-xs h-6 min-h-0 w-6 p-0 text-gray-400 hover:bg-red-50 hover:text-red-500 dark:hover:bg-red-900/20"
                                                                                        title="删除目标模型"
                                                                                        aria-label="删除目标模型"
                                                                                    >
                                                                                        <Trash2 size={12} />
                                                                                    </button>
                                                                                )}
                                                                            </div>
                                                                        </div>
                                                                    </div>
                                                                );
                                                            })}
                                                        </div>
                                                    </section>
                                                );
                                            })}
                                        </div>
                                    ) : (
                                        <div className="py-6 text-center text-[11px] italic text-gray-500 dark:text-gray-400">
                                            {t('proxy.router.no_custom_mapping')}
                                        </div>
                                    )}
                                </div>
                            </div>

                            {/* Weighted Mapping Add Form */}
                            <div className="w-full rounded-xl border border-gray-200 bg-white p-2.5 shadow-inner dark:border-base-300 dark:bg-base-100">
                                <div className="mb-2 flex items-center gap-2">
                                    <Zap size={12} className="text-amber-500" />
                                    <span className="text-[10px] font-bold uppercase tracking-wider text-gray-500 dark:text-gray-400">
                                        1对N 权重映射
                                    </span>
                                    <span className="ml-auto text-[10px] text-gray-500 dark:text-gray-400">
                                        单次添加一个目标，重复添加同一模板即可增加模型
                                    </span>
                                </div>

                                <div className="grid grid-cols-1 items-center gap-2 lg:grid-cols-[minmax(12rem,16rem)_minmax(0,1fr)_7rem_auto]">
                                    <GroupedSelect
                                        value={weightedKey}
                                        onChange={setWeightedKey}
                                        options={weightedSourceModelTemplates}
                                        placeholder="选择原始模型模板或自定义输入"
                                        className="h-8 w-full"
                                        allowCustomInput={true}
                                    />
                                    <ProviderModelSelect
                                        value={weightedTargets[0]?.target || ''}
                                        onChange={v => {
                                            updateWeightedTarget(0, 'target', v);
                                            setWeightedEffort('');
                                        }}
                                        sources={modelSources}
                                        placeholder="目标模型"
                                        className="h-8 w-full min-w-0 font-mono text-[11px] dark:bg-base-100 lg:flex-nowrap"
                                        allowCustomInput={true}
                                    />
                                    <ReasoningEffortSelect
                                        value={weightedEffort}
                                        target={weightedTargets[0]?.target || ''}
                                        sources={modelSources}
                                        onChange={setWeightedEffort}
                                        className="w-full min-w-0"
                                        disabled={!weightedTargets[0]?.target}
                                    />
                                    <button className="btn btn-xs w-full gap-1.5 bg-blue-600 text-white shadow-md transition-all hover:bg-blue-700 hover:shadow-lg lg:w-20" onClick={handleAddWeightedMapping}><Plus size={14} />{t('common.add')}</button>
                                </div>
                            </div>
                        </div>
                    </div>
                </div>

                {/* Fallback Model Panel */}
                <div className="bg-white dark:bg-base-100 rounded-xl p-4 border border-gray-100 dark:border-base-200 shadow-sm">
                    <div className="flex items-center gap-3 mb-4">
                        <div className="p-1.5 bg-amber-50 dark:bg-amber-900/20 rounded text-amber-600 dark:text-amber-400">
                            <ShieldAlert size={20} />
                        </div>
                        <div className="flex-1">
                            <h3 className="text-base font-bold text-gray-900 dark:text-gray-100 leading-none">兜底模型</h3>
                            <p className="text-[11px] text-gray-500 dark:text-gray-400 mt-1">请求模型无法匹配有效供应商和模型时，改用指定的兜底模型</p>
                        </div>
                        <label className="relative inline-flex items-center cursor-pointer">
                            <input type="checkbox" className="sr-only peer" checked={fallbackEnabled} onChange={e => setFallbackEnabled(e.target.checked)} />
                            <div className="w-11 h-6 bg-gray-200 dark:bg-base-300 peer-focus:outline-none rounded-full peer peer-checked:after:translate-x-full peer-checked:after:border-white after:content-[''] after:absolute after:top-[2px] after:left-[2px] after:bg-white after:border-gray-300 after:border after:rounded-full after:h-5 after:w-5 after:transition-all peer-checked:bg-amber-500 shadow-inner"></div>
                        </label>
                    </div>
                    {fallbackEnabled && (
                        <div className="grid grid-cols-1 gap-3 lg:grid-cols-[minmax(0,1fr)_9rem_auto]">
                            <div className="min-w-0">
                                <label className="text-[10px] text-gray-400 mb-1 block">兜底模型</label>
                                <ProviderModelSelect
                                    value={fallbackModel}
                                    onChange={(value) => {
                                        setFallbackModel(value);
                                        setFallbackEffort('');
                                        const providerSource = modelSources.find((source) => source.kind === 'provider' && value.startsWith(`${source.targetPrefix}/`));
                                        setFallbackProviderId(providerSource?.targetPrefix ?? '');
                                    }}
                                    sources={modelSources}
                                    placeholder="输入或选择模型"
                                    className="font-mono text-[11px] h-9 dark:bg-base-200 w-full"
                                    allowCustomInput={true}
                                />
                            </div>
                            <div className="min-w-0">
                                <label className="text-[10px] text-gray-400 mb-1 block">思考强度</label>
                                <ReasoningEffortSelect
                                    value={fallbackEffort}
                                    target={fallbackModel}
                                    sources={modelSources}
                                    onChange={setFallbackEffort}
                                    className="h-9 w-full min-w-0"
                                    disabled={!fallbackModel}
                                />
                            </div>
                            <div className="flex items-end">
                                <button className="btn btn-sm w-full bg-amber-600 hover:bg-amber-700 text-white gap-1 h-9 lg:w-20" onClick={handleSaveFallbackModel}><Save size={14} />保存</button>
                            </div>
                        </div>
                    )}
                    {fallbackEnabled && (
                        <p className="mt-3 text-[11px] text-gray-500 dark:text-gray-400">
                            仅在路由映射后的目标没有匹配到供应商或可用模型时生效；关闭后保留原有路由行为。思考强度留空表示跟随请求。
                        </p>
                    )}
                </div>

                {/* Cooldown Status Panel */}
                <div className="bg-white dark:bg-base-100 rounded-xl p-4 border border-gray-100 dark:border-base-200 shadow-sm">
                    <div className="flex items-center gap-3 mb-4">
                        <div className="p-1.5 bg-red-50 dark:bg-red-900/20 rounded text-red-600 dark:text-red-400">
                            <Clock size={20} />
                        </div>
                        <div className="flex-1">
                            <h3 className="text-base font-bold text-gray-900 dark:text-gray-100 leading-none">模型冷却状态</h3>
                            <p className="text-[11px] text-gray-500 dark:text-gray-400 mt-1">429 后暂时排除对应供应商、协议和模型，倒计时结束自动恢复</p>
                        </div>
                        <div className="flex items-center gap-1">
                            <span className="text-[10px] text-gray-400">冷却时长</span>
                            <input type="number" min={30} max={3600} value={cooldownDuration} onChange={e => setCooldownDuration(parseInt(e.target.value) || 600)} className="input input-xs input-bordered w-20 font-mono text-[11px] bg-white dark:bg-base-100 h-7" />
                            <span className="text-[10px] text-gray-400">秒</span>
                            <button className="btn btn-xs btn-square bg-red-600 hover:bg-red-700 text-white h-7" onClick={handleSaveCooldownConfig}><Save size={12} /></button>
                        </div>
                        <label className="relative inline-flex items-center cursor-pointer">
                            <input type="checkbox" className="sr-only peer" checked={cooldownEnabled} onChange={e => handleToggleCooldown(e.target.checked)} />
                            <div className="w-11 h-6 bg-gray-200 dark:bg-base-300 peer-focus:outline-none rounded-full peer peer-checked:after:translate-x-full peer-checked:after:border-white after:content-[''] after:absolute after:top-[2px] after:left-[2px] after:bg-white after:border-gray-300 after:border after:rounded-full after:h-5 after:w-5 after:transition-all peer-checked:bg-red-500 shadow-inner"></div>
                        </label>
                    </div>

                    <div className="space-y-3">

                        {cooldowns.length > 0 ? (
                                <div className="space-y-1">
                                    {cooldowns.map((cd, idx) => (
                                        <div key={`${cd.model}-${cd.provider}-${cd.protocol}-${idx}`} className="flex items-center justify-between px-3 py-2 bg-red-50/50 dark:bg-red-900/10 rounded-md border border-red-100 dark:border-red-900/30">
                                            <div className="flex items-center gap-3">
                                                <Activity size={12} className="text-red-500 animate-pulse" />
                                                <span className="font-mono text-[11px] text-gray-700 dark:text-gray-300">{cd.model}</span>
                                                <span className="text-[10px] text-gray-500 dark:text-gray-400">@{cd.provider} · {cd.protocol}</span>
                                            </div>
                                            <span className="font-mono text-[11px] text-red-600 dark:text-red-400">{cd.remaining_secs}s</span>
                                        </div>
                                    ))}
                                    <button className="btn btn-xs btn-ghost text-error gap-1 mt-2" onClick={handleClearCooldowns}><Trash2 size={12} />全部清除</button>
                                </div>
                                ) : (
                                    <div className="text-center py-4 text-gray-400 dark:text-gray-600 italic text-[11px]">当前没有处于冷却状态的模型</div>
                                )}
                        </div>
                </div>

                {/* Experimental Settings */}
                <div className="bg-white dark:bg-base-100 rounded-xl p-4 border border-gray-100 dark:border-base-200 shadow-sm">
                    <div className="flex items-center gap-3 mb-4">
                        <div className="p-1.5 bg-purple-50 dark:bg-purple-900/20 rounded text-purple-600 dark:text-purple-400">
                            <Sparkles size={20} />
                        </div>
                        <div>
                            <h3 className="text-base font-bold text-gray-900 dark:text-gray-100 leading-none">
                                {t('proxy.config.experimental.title')}
                            </h3>
                            <p className="text-[11px] text-gray-500 dark:text-gray-400 mt-1">
                                {t('proxy.config.experimental.description', { defaultValue: '实验性设置，可能影响稳定性' })}
                            </p>
                        </div>
                    </div>

                    <div className="space-y-4">
                        {/* Enable Usage Scaling */}
                        <div className="flex items-center justify-between p-4 bg-gray-50 dark:bg-base-200 rounded-xl border border-gray-100 dark:border-base-300">
                            <div className="space-y-1">
                                <div className="flex items-center gap-2">
                                    <span className="text-sm font-bold text-gray-900 dark:text-base-content">
                                        {t('proxy.config.experimental.enable_usage_scaling')}
                                    </span>
                                    <HelpTooltip text={t('proxy.config.experimental.enable_usage_scaling_tooltip')} />
                                    <span className="px-1.5 py-0.5 rounded bg-purple-100 dark:bg-purple-900/30 text-[10px] text-purple-600 dark:text-purple-400 font-bold border border-purple-200 dark:border-purple-800">
                                        Claude
                                    </span>
                                </div>
                                <p className="text-[10px] text-gray-500 dark:text-gray-400 max-w-lg">
                                    {t('proxy.config.experimental.enable_usage_scaling_tooltip')}
                                </p>
                            </div>
                            <label className="relative inline-flex items-center cursor-pointer">
                                <input
                                    type="checkbox"
                                    className="sr-only peer"
                                    checked={!!appConfig.proxy.experimental?.enable_usage_scaling}
                                    onChange={(e) => updateExperimentalConfig({ enable_usage_scaling: e.target.checked })}
                                />
                                <div className="w-11 h-6 bg-gray-200 dark:bg-base-300 peer-focus:outline-none rounded-full peer peer-checked:after:translate-x-full peer-checked:after:border-white after:content-[''] after:absolute after:top-[2px] after:left-[2px] after:bg-white after:border-gray-300 after:border after:rounded-full after:h-5 after:w-5 after:transition-all peer-checked:bg-purple-500 shadow-inner"></div>
                            </label>
                        </div>

                        {/* L1 Threshold */}
                        <div className="flex flex-col gap-2 p-4 bg-gray-50 dark:bg-base-200 rounded-xl border border-gray-100 dark:border-base-300">
                            <div className="flex items-center justify-between w-full">
                                <div className="flex items-center gap-2">
                                    <span className="text-sm font-bold text-gray-900 dark:text-base-content">
                                        {t('proxy.config.experimental.context_compression_threshold_l1')}
                                    </span>
                                    <HelpTooltip text={t('proxy.config.experimental.context_compression_threshold_l1_tooltip')} />
                                </div>
                            </div>
                            <DebouncedSlider
                                min={0.1}
                                max={1}
                                step={0.05}
                                className="range range-purple range-xs"
                                value={appConfig.proxy.experimental?.context_compression_threshold_l1 || 0.4}
                                onChange={(val) => updateExperimentalConfig({ context_compression_threshold_l1: val })}
                            />
                        </div>

                        {/* L2 Threshold */}
                        <div className="flex flex-col gap-2 p-4 bg-gray-50 dark:bg-base-200 rounded-xl border border-gray-100 dark:border-base-300">
                            <div className="flex items-center justify-between w-full">
                                <div className="flex items-center gap-2">
                                    <span className="text-sm font-bold text-gray-900 dark:text-base-content">
                                        {t('proxy.config.experimental.context_compression_threshold_l2')}
                                    </span>
                                    <HelpTooltip text={t('proxy.config.experimental.context_compression_threshold_l2_tooltip')} />
                                </div>
                            </div>
                            <DebouncedSlider
                                min={0.1}
                                max={1}
                                step={0.05}
                                className="range range-purple range-xs"
                                value={appConfig.proxy.experimental?.context_compression_threshold_l2 || 0.55}
                                onChange={(val) => updateExperimentalConfig({ context_compression_threshold_l2: val })}
                            />
                        </div>

                        {/* L3 Threshold */}
                        <div className="flex flex-col gap-2 p-4 bg-gray-50 dark:bg-base-200 rounded-xl border border-gray-100 dark:border-base-300">
                            <div className="flex items-center justify-between w-full">
                                <div className="flex items-center gap-2">
                                    <span className="text-sm font-bold text-gray-900 dark:text-base-content">
                                        {t('proxy.config.experimental.context_compression_threshold_l3')}
                                    </span>
                                    <HelpTooltip text={t('proxy.config.experimental.context_compression_threshold_l3_tooltip')} />
                                </div>
                            </div>
                            <DebouncedSlider
                                min={0.1}
                                max={1}
                                step={0.05}
                                className="range range-purple range-xs"
                                value={appConfig.proxy.experimental?.context_compression_threshold_l3 || 0.7}
                                onChange={(val) => updateExperimentalConfig({ context_compression_threshold_l3: val })}
                            />
                        </div>
                    </div>
                </div>

                {/* Account Scheduling & Rotation — hidden until account system integration */}
                <div className="bg-white dark:bg-base-100 rounded-xl p-4 border border-gray-100 dark:border-base-200 shadow-sm hidden">
                    <div className="flex items-center gap-3 mb-4">
                        <div className="p-1.5 bg-indigo-50 dark:bg-indigo-900/20 rounded text-indigo-600 dark:text-indigo-400">
                            <RefreshCw size={20} />
                        </div>
                        <div>
                            <h3 className="text-base font-bold text-gray-900 dark:text-gray-100 leading-none">
                                {t('proxy.config.scheduling.title')}
                            </h3>
                            <p className="text-[11px] text-gray-500 dark:text-gray-400 mt-1">
                                {t('proxy.config.scheduling.subtitle', { defaultValue: '账号轮换与会话调度策略' })}
                            </p>
                        </div>
                    </div>

                    <div className="space-y-4">
                        {/* Scheduling Mode */}
                        <div className="grid grid-cols-1 md:grid-cols-2 gap-6">
                            <div className="space-y-3">
                                <div className="flex items-center justify-between">
                                    <label className="text-xs font-medium text-gray-700 dark:text-gray-300 inline-flex items-center gap-1">
                                        {t('proxy.config.scheduling.mode')}
                                        <HelpTooltip text={t('proxy.config.scheduling.mode_tooltip')} placement="right" />
                                    </label>
                                    <button
                                        onClick={handleClearSessionBindings}
                                        className="text-[10px] text-indigo-500 hover:text-indigo-600 transition-colors flex items-center gap-1"
                                        title={t('proxy.config.scheduling.clear_bindings_tooltip', { defaultValue: 'Clear all session-to-account bindings' })}
                                    >
                                        <Trash2 size={12} />
                                        {t('proxy.config.scheduling.clear_bindings', { defaultValue: 'Clear Bindings' })}
                                    </button>
                                </div>
                                <div className="grid grid-cols-1 gap-2">
                                    {(['CacheFirst', 'Balance', 'PerformanceFirst'] as const).map(mode => (
                                        <label
                                            key={mode}
                                            className={`flex items-start gap-3 p-3 rounded-xl border cursor-pointer transition-all duration-200 ${(appConfig.proxy.scheduling?.mode || 'Balance') === mode
                                                ? 'border-indigo-500 bg-indigo-50/30 dark:bg-indigo-900/10'
                                                : 'border-gray-100 dark:border-base-200 hover:border-indigo-200'
                                                }`}
                                        >
                                            <input
                                                type="radio"
                                                className="radio radio-xs radio-primary mt-1"
                                                checked={(appConfig.proxy.scheduling?.mode || 'Balance') === mode}
                                                onChange={() => updateSchedulingConfig({ mode })}
                                            />
                                            <div className="space-y-1">
                                                <div className="text-xs font-bold text-gray-900 dark:text-base-content">
                                                    {t(`proxy.config.scheduling.modes.${mode}`)}
                                                </div>
                                                <div className="text-[10px] text-gray-500 line-clamp-2">
                                                    {t(`proxy.config.scheduling.modes_desc.${mode}`, {
                                                        defaultValue: mode === 'CacheFirst' ? 'Binds session to account, waits precisely if limited (Maximizes Prompt Cache hits).' :
                                                            mode === 'Balance' ? 'Binds session, auto-switches to available account if limited (Balanced cache & availability).' :
                                                                'No session binding, pure round-robin rotation (Best for high concurrency).'
                                                    })}
                                                </div>
                                            </div>
                                        </label>
                                    ))}
                                </div>
                            </div>

                            <div className="space-y-4 pt-1">
                                {/* Max Wait Slider */}
                                <div className="bg-slate-100 dark:bg-slate-800/80 rounded-xl p-4 border border-slate-200 dark:border-slate-700">
                                    <div className="flex items-center justify-between mb-2">
                                        <label className="text-xs font-medium text-gray-700 dark:text-gray-300 inline-flex items-center gap-1">
                                            {t('proxy.config.scheduling.max_wait')}
                                            <HelpTooltip text={t('proxy.config.scheduling.max_wait_tooltip', { defaultValue: 'Maximum wait time for CacheFirst mode when the bound account is rate-limited.' })} />
                                        </label>
                                        <span className="text-xs font-mono text-indigo-600 font-bold">
                                            {appConfig.proxy.scheduling?.max_wait_seconds || 60}s
                                        </span>
                                    </div>
                                    <input
                                        type="range"
                                        min="0"
                                        max="300"
                                        step="10"
                                        disabled={(appConfig.proxy.scheduling?.mode || 'Balance') !== 'CacheFirst'}
                                        className="range range-indigo range-xs"
                                        value={appConfig.proxy.scheduling?.max_wait_seconds || 60}
                                        onChange={(e) => updateSchedulingConfig({ max_wait_seconds: parseInt(e.target.value) })}
                                    />
                                    <div className="flex justify-between px-1 mt-1 text-[10px] text-gray-400 font-mono">
                                        <span>0s</span>
                                        <span>300s</span>
                                    </div>
                                </div>

                                <div className="p-3 bg-amber-50 dark:bg-amber-900/10 border border-amber-100 dark:border-amber-900/20 rounded-xl">
                                    <p className="text-[10px] text-amber-700 dark:text-amber-500 leading-relaxed">
                                        <strong>{t('common.info')}:</strong> {t('proxy.config.scheduling.subtitle', { defaultValue: '调度策略影响会话绑定的行为' })}
                                    </p>
                                </div>

                                {/* Fixed Account Mode */}
                                <div className="bg-indigo-50 dark:bg-indigo-900/20 rounded-xl p-4 border border-indigo-200 dark:border-indigo-800">
                                    <div className="flex items-center justify-between mb-3">
                                        <label className="text-xs font-medium text-gray-700 dark:text-gray-300 inline-flex items-center gap-1">
                                            🔒 {t('proxy.config.scheduling.fixed_account', { defaultValue: 'Fixed Account Mode' })}
                                            <HelpTooltip text={t('proxy.config.scheduling.fixed_account_tooltip', { defaultValue: 'When enabled, all API requests will use only the selected account instead of rotating between accounts.' })} />
                                        </label>
                                        <input
                                            type="checkbox"
                                            className="toggle toggle-sm toggle-primary"
                                            checked={preferredAccountId !== null}
                                            onChange={(e) => {
                                                if (e.target.checked) {
                                                    if (availableAccounts.length > 0) {
                                                        handleSetPreferredAccount(availableAccounts[0].id);
                                                    }
                                                } else {
                                                    handleSetPreferredAccount(null);
                                                }
                                            }}
                                            disabled={!status.running}
                                        />
                                    </div>
                                    {preferredAccountId !== null && (
                                        <select
                                            className="select select-bordered select-sm w-full text-xs"
                                            value={preferredAccountId || ''}
                                            onChange={(e) => handleSetPreferredAccount(e.target.value || null)}
                                            disabled={!status.running}
                                        >
                                            {availableAccounts.map(account => (
                                                <option key={account.id} value={account.id}>
                                                    {account.email}
                                                </option>
                                            ))}
                                        </select>
                                    )}
                                    {!status.running && (
                                        <p className="text-[10px] text-gray-500 mt-2">
                                            {t('proxy.config.scheduling.start_service_first', { defaultValue: 'Start the service to configure fixed account mode.' })}
                                        </p>
                                    )}
                                </div>
                            </div>
                        </div>

                        {/* Circuit Breaker */}
                        {appConfig.circuit_breaker && (
                            <div className="pt-4 border-t border-gray-100 dark:border-gray-700/50">
                                <div className="flex items-center justify-between mb-4">
                                    <label className="text-xs font-medium text-gray-700 dark:text-gray-300 inline-flex items-center gap-1">
                                        {t('proxy.config.circuit_breaker.title', { defaultValue: 'Adaptive Circuit Breaker' })}
                                        <HelpTooltip text={t('proxy.config.circuit_breaker.tooltip', { defaultValue: 'Prevent continuous failures by exponentially backing off when quota is exhausted.' })} />
                                    </label>
                                    <input
                                        type="checkbox"
                                        className="toggle toggle-sm toggle-warning"
                                        checked={appConfig.circuit_breaker.enabled}
                                        onChange={(e) => {
                                            if (!appConfig) return;
                                            const newConfig = {
                                                ...appConfig,
                                                circuit_breaker: { ...appConfig.circuit_breaker, enabled: e.target.checked }
                                            };
                                            saveConfig(newConfig);
                                        }}
                                    />
                                </div>

                                {appConfig.circuit_breaker.enabled && (
                                    <CircuitBreaker
                                        config={appConfig.circuit_breaker}
                                        onChange={(newBreakerConfig) => {
                                            if (!appConfig) return;
                                            const newConfig = {
                                                ...appConfig,
                                                circuit_breaker: newBreakerConfig
                                            };
                                            saveConfig(newConfig);
                                        }}
                                        onClearRateLimits={async () => {
                                            try {
                                                await invoke('clear_local_rate_limits');
                                                showToast(t('common.success'), 'success');
                                            } catch (error) {
                                                console.error('Failed to clear rate limits:', error);
                                                showToast(`${t('common.error')}: ${error}`, 'error');
                                            }
                                        }}
                                    />
                                )}
                            </div>
                        )}
                    </div>
                </div>

                {/* Modals */}
                <ModalDialog
                    isOpen={isResetConfirmOpen}
                    title={t('proxy.router.reset_mapping')}
                    message={t('proxy.dialog.reset_mapping_confirm') || '确定要重置所有映射吗？这将清除所有自定义映射。'}
                    type="confirm"
                    isDestructive={true}
                    onConfirm={executeResetMapping}
                    onCancel={() => setIsResetConfirmOpen(false)}
                />

                <ModalDialog
                    isOpen={isClearBindingsConfirmOpen}
                    title={t('proxy.dialog.clear_bindings_title') || 'Clear Session Bindings'}
                    message={t('proxy.dialog.clear_bindings_msg') || 'Are you sure you want to clear all session-to-account binding mappings?'}
                    type="confirm"
                    isDestructive={true}
                    onConfirm={executeClearSessionBindings}
                    onCancel={() => setIsClearBindingsConfirmOpen(false)}
                />

                <ModalDialog
                    isOpen={isPresetManagerOpen}
                    title={t('proxy.router.manage_presets_title')}
                    onConfirm={() => setIsPresetManagerOpen(false)}
                    confirmText={t('common.close')}
                    type="info"
                >
                    <div className="space-y-6">
                        {/* Save Current Section */}
                        <div className="space-y-3 p-4 bg-blue-50/50 dark:bg-blue-900/10 rounded-xl border border-blue-100 dark:border-blue-900/20">
                            <h3 className="text-sm font-bold text-gray-800 dark:text-gray-200 flex items-center gap-2">
                                <Save size={16} className="text-blue-500" />
                                {t('proxy.router.save_current_as_preset')}
                            </h3>
                            <div className="flex gap-2">
                                <input
                                    type="text"
                                    value={newPresetName}
                                    onChange={(e) => setNewPresetName(e.target.value)}
                                    placeholder={t('proxy.router.preset_name_placeholder')}
                                    className="input input-sm flex-1 border-gray-300 focus:border-blue-500"
                                />
                                <button
                                    onClick={handleSaveCurrentAsPreset}
                                    disabled={!newPresetName.trim()}
                                    className="btn btn-sm btn-primary text-white"
                                >
                                    {t('common.save')}
                                </button>
                            </div>
                            <p className="text-[10px] text-gray-500 dark:text-gray-400">
                                {t('proxy.router.save_hint')}
                            </p>
                        </div>

                        {/* Existing Presets List */}
                        <div className="space-y-3">
                            <h3 className="text-sm font-bold text-gray-800 dark:text-gray-200 px-1">
                                {t('proxy.router.your_presets')}
                            </h3>
                            <div className="max-h-[300px] overflow-y-auto space-y-2 pr-1">
                                {customPresets.length === 0 ? (
                                    <div className="text-center py-8 text-gray-400 dark:text-gray-600 bg-gray-50 dark:bg-base-200 rounded-xl border border-dashed border-gray-200 dark:border-gray-700">
                                        <p>{t('proxy.router.no_custom_presets')}</p>
                                    </div>
                                ) : (
                                    customPresets.map(preset => (
                                        <div key={preset.id} className="flex items-center justify-between p-3 bg-white dark:bg-base-200 border border-gray-100 dark:border-gray-700 rounded-xl hover:shadow-sm transition-all group">
                                            <div className="flex-1 min-w-0">
                                                <div className="font-bold text-sm text-gray-800 dark:text-gray-200 truncate">{preset.name}</div>
                                                <div className="text-[10px] text-gray-400 dark:text-gray-500 truncate">
                                                    {Object.keys(preset.mappings).length} {t('proxy.router.mappings_count')}
                                                </div>
                                            </div>
                                            <div className="flex items-center gap-2 opacity-0 group-hover:opacity-100 transition-opacity">
                                                <button
                                                    onClick={() => handleDeletePreset(preset.id)}
                                                    className="p-1.5 text-gray-400 hover:text-red-500 hover:bg-red-50 dark:hover:bg-red-900/20 rounded-lg transition-colors"
                                                    title={t('common.delete')}
                                                >
                                                    <Trash2 size={16} />
                                                </button>
                                            </div>
                                        </div>
                                    ))
                                )}
                            </div>
                        </div>
                    </div>
                </ModalDialog>
            </div>
        </div>
    );
}
