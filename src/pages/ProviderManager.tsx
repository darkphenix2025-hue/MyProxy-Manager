import { useState, useCallback, useMemo, useRef, useEffect } from 'react';
import { useTranslation } from 'react-i18next';
import { createPortal } from 'react-dom';
import {
    Plus,
    Search,
    Edit2,
    Trash2,
    Server,
    AlertCircle,
    X,
    Eye,
    EyeOff,
    FlaskConical,
    Loader2,
    CheckCircle2,
    XCircle,
    RotateCcw,
    RefreshCw,
    ChevronDown,
    ChevronUp,
    Image,
    SlidersHorizontal,
} from 'lucide-react';
import { useConfigStore } from '../stores/useConfigStore';
import { showToast } from '../components/common/ToastContainer';
import ModalDialog from '../components/common/ModalDialog';
import GroupedSelect, { type SelectOption } from '../components/common/GroupedSelect';
import { request } from '../utils/request';
import {
    codexModelCatalogKey,
    getModelCatalog,
    providerModelCatalogKey,
    setModelCatalog,
} from '../services/modelCatalogCache';
import type {
    UpstreamProvider,
    ProviderModelConfig,
    ProviderProtocolConfig,
    ProviderProtocol,
    ProviderDispatchMode,
} from '../types/config';
import type { CodexConnection } from '../types/codex';
import {
    groupProviders,
    legacyProtocolConfig,
    logicalProviderName,
    modelConfigsForProvider,
    protocolConfigsForProvider,
} from '../utils/providerGrouping';

interface ModelTestResult {
    model: string;
    protocol?: string;
    success: boolean;
    latency_ms: number;
    latencyMs?: number;
    error?: string;
}

interface ProviderTestDialogState {
    provider: UpstreamProvider;
    protocol: ProviderProtocol;
    model: string;
}

interface DiscoveredProviderModel {
    id: string;
    name?: string | null;
    display_name?: string | null;
    displayName?: string | null;
    owned_by?: string | null;
}

const MODEL_REASONING_OPTIONS = [
    { value: 'none', label: '关闭思考' },
    { value: 'minimal', label: '最少 / minimal' },
    { value: 'low', label: '低 / low' },
    { value: 'medium', label: '中 / medium' },
    { value: 'high', label: '高 / high' },
    { value: 'xhigh', label: '超高 / xhigh' },
    { value: 'max', label: '最大 / max' },
    { value: 'ultra', label: '极限 / ultra' },
    { value: 'auto', label: '自动 / auto' },
] as const;

const ALL_TEST_MODELS = '__all_models__';

function normalizeDiscoveredModels(value: unknown): ProviderModelConfig[] {
    if (!Array.isArray(value)) return [];
    const byId = new Map<string, ProviderModelConfig>();
    for (const item of value) {
        const record = typeof item === 'object' && item !== null
            ? item as DiscoveredProviderModel
            : null;
        const id = (typeof item === 'string' ? item : record?.id || '').trim();
        if (!id || byId.has(id)) continue;
        const displayName = (
            record?.display_name || record?.displayName || record?.name || ''
        ).trim();
        byId.set(id, {
            id,
            ...(displayName ? { display_name: displayName } : {}),
        });
    }
    return Array.from(byId.values()).sort((left, right) => left.id.localeCompare(right.id));
}

const PROTOCOL_OPTIONS: { value: ProviderProtocol; label: string }[] = [
    { value: 'anthropic_passthrough', label: 'Anthropic' },
    { value: 'open_a_i_compatible' as ProviderProtocol, label: 'OpenAI' },
    { value: 'codex_responses', label: 'Codex' },
    { value: 'gemini_v1_internal' as ProviderProtocol, label: 'Gemini' },
];

const DISPATCH_OPTIONS: { value: ProviderDispatchMode; label: string }[] = [
    { value: 'exclusive', label: 'Exclusive' },
    { value: 'pooled', label: 'Pooled' },
    { value: 'fallback', label: 'Fallback' },
];

const PROTOCOL_COLORS: Record<ProviderProtocol, string> = {
    anthropic_passthrough: 'bg-blue-100 dark:bg-blue-900/30 text-blue-700 dark:text-blue-300 border-blue-200 dark:border-blue-800',
    open_a_i_compatible: 'bg-green-100 dark:bg-green-900/30 text-green-700 dark:text-green-300 border-green-200 dark:border-green-800',
    codex_responses: 'bg-purple-100 dark:bg-purple-900/30 text-purple-700 dark:text-purple-300 border-purple-200 dark:border-purple-800',
    gemini_v1_internal: 'bg-orange-100 dark:bg-orange-900/30 text-orange-700 dark:text-orange-300 border-orange-200 dark:border-orange-800',
};

function enabledTestProtocols(provider: UpstreamProvider): ProviderProtocolConfig[] {
    return protocolConfigsForProvider(provider).filter((config) => config.enabled);
}

function testModelOptions(provider: UpstreamProvider): SelectOption[] {
    return modelConfigsForProvider(provider).map((model) => {
        const displayName = model.alias?.trim() || model.display_name?.trim();
        return {
            value: model.id,
            label: displayName && displayName !== model.id
                ? `${displayName} (${model.id})`
                : model.id,
            group: '可用模型',
        };
    });
}

const PROTOCOL_CONFIG_FIELDS = new Set<keyof ProviderProtocolConfig>([
    'base_url',
    'api_key',
    'credential_id',
    'dispatch_mode',
    'priority',
    'model_prefixes',
    'model_mapping',
    'request_timeout_secs',
]);

function createEmptyProvider(): UpstreamProvider {
    return {
        name: '',
        provider_id: '',
        enabled: true,
        base_url: 'https://',
        api_key: '',
        protocol: 'anthropic_passthrough',
        dispatch_mode: 'pooled',
        priority: 0,
        model_prefixes: [],
        model_mapping: {},
        available_models: '',
        model_configs: [],
    };
}

/* ─── Custom Modal Dialog for Add/Edit ─── */

interface ProviderFormModalProps {
    isOpen: boolean;
    editingProvider: UpstreamProvider | null;
    initialData: UpstreamProvider;
    codexConnections: CodexConnection[];
    onConfirm: (
        values: UpstreamProvider,
        mappings: { from: string; to: string }[],
        modelConfigs: ProviderModelConfig[],
    ) => void;
    onCancel: () => void;
}

function ProviderFormModal({
    isOpen,
    editingProvider,
    initialData,
    codexConnections,
    onConfirm,
    onCancel,
}: ProviderFormModalProps) {
    const { t } = useTranslation();
    const [formData, setFormData] = useState<UpstreamProvider>(initialData);
    const [modelMappings, setModelMappings] = useState<{ from: string; to: string }[]>([]);
    const [newMappingFrom, setNewMappingFrom] = useState('');
    const [newMappingTo, setNewMappingTo] = useState('');
    const [showMappingForm, setShowMappingForm] = useState(false);
    const [showAdvanced, setShowAdvanced] = useState(true);
    const [showApiKey, setShowApiKey] = useState(false);
    const [fetchingModels, setFetchingModels] = useState(false);
    const [useOpenAIForModelDiscovery, setUseOpenAIForModelDiscovery] = useState(false);
    const [modelConfigs, setModelConfigs] = useState<ProviderModelConfig[]>([]);
    const [protocolConfigs, setProtocolConfigs] = useState<ProviderProtocolConfig[]>([]);
    const [activeProtocol, setActiveProtocol] = useState<ProviderProtocol>(initialData.protocol);
    const [expandedModels, setExpandedModels] = useState<Set<string>>(new Set());
    const [modelCatalog, setModelCatalogState] = useState<ProviderModelConfig[]>([]);
    const [modelCatalogQuery, setModelCatalogQuery] = useState('');
    const [selectedCatalogModels, setSelectedCatalogModels] = useState<Set<string>>(new Set());
    const [showModelPicker, setShowModelPicker] = useState(false);
    const nameInputRef = useRef<HTMLInputElement>(null);

    useEffect(() => {
        const initialProtocols = protocolConfigsForProvider(initialData);
        const initialActiveProtocol = initialProtocols.find((config) => (
            config.enabled && config.protocol === initialData.protocol
        ))?.protocol || initialProtocols.find((config) => config.enabled)?.protocol || initialData.protocol;
        const initialActiveConfig = initialProtocols.find((config) => config.protocol === initialActiveProtocol)
            || initialProtocols[0];
        setProtocolConfigs(initialProtocols);
        setActiveProtocol(initialActiveProtocol);
        setFormData({
            ...initialData,
            ...initialActiveConfig,
            protocol: initialActiveConfig.protocol,
            protocols: initialProtocols,
        });
        setModelMappings(
            Object.entries(initialActiveConfig.model_mapping || {}).map(([from, to]) => ({ from, to }))
        );
        setShowMappingForm(false);
        setNewMappingFrom('');
        setNewMappingTo('');
        setShowAdvanced(true);
        setShowApiKey(false);
        setFetchingModels(false);
        setUseOpenAIForModelDiscovery(false);
        const initialModelConfigs = modelConfigsForProvider(initialData);
        setModelConfigs(initialModelConfigs);
        setExpandedModels(new Set());
        setModelCatalogQuery('');
        setSelectedCatalogModels(new Set());
        setShowModelPicker(false);

        const catalogKey = initialData.provider_id?.trim()
            ? providerModelCatalogKey(initialData.provider_id.trim())
            : initialData.credential_id?.trim()
                ? codexModelCatalogKey(initialData.credential_id.trim())
                : null;
        const cachedCatalog = catalogKey ? getModelCatalog(catalogKey) : null;
        setModelCatalogState(
            cachedCatalog?.models.map((model) => ({
                id: model.id,
                ...(model.displayName ? { display_name: model.displayName } : {}),
            })) || [],
        );
    }, [initialData]);

    useEffect(() => {
        if (isOpen && nameInputRef.current) {
            setTimeout(() => nameInputRef.current?.focus(), 100);
        }
    }, [isOpen]);

    if (!isOpen) return null;

    const updateProtocolField = <K extends keyof ProviderProtocolConfig>(
        field: K,
        value: ProviderProtocolConfig[K],
    ) => {
        setProtocolConfigs((current) => current.map((config) => (
            config.protocol === activeProtocol ? { ...config, [field]: value } : config
        )));
    };

    const updateField = <K extends keyof UpstreamProvider>(field: K, value: UpstreamProvider[K]) => {
        setFormData(prev => ({ ...prev, [field]: value }));
        if (PROTOCOL_CONFIG_FIELDS.has(field as keyof ProviderProtocolConfig)) {
            updateProtocolField(
                field as keyof ProviderProtocolConfig,
                value as ProviderProtocolConfig[keyof ProviderProtocolConfig],
            );
        }
    };

    const activateProtocolConfig = (config: ProviderProtocolConfig) => {
        setActiveProtocol(config.protocol);
        setFormData((current) => ({
            ...current,
            ...config,
            protocol: config.protocol,
            protocols: protocolConfigs,
        }));
        setModelMappings(
            Object.entries(config.model_mapping || {}).map(([from, to]) => ({ from, to })),
        );
        setUseOpenAIForModelDiscovery(false);
    };

    const selectProtocol = (protocol: ProviderProtocol) => {
        const config = protocolConfigs.find((item) => item.protocol === protocol && item.enabled);
        if (config) activateProtocolConfig(config);
    };

    const toggleProtocol = (protocol: ProviderProtocol, enabled: boolean) => {
        const enabledCount = protocolConfigs.filter((config) => config.enabled).length;
        if (!enabled && enabledCount <= 1) {
            showToast('至少保留一个启用协议', 'warning');
            return;
        }

        let nextConfigs = protocolConfigs;
        const existing = protocolConfigs.find((config) => config.protocol === protocol);
        if (existing) {
            nextConfigs = protocolConfigs.map((config) => (
                config.protocol === protocol ? { ...config, enabled } : config
            ));
        } else if (enabled) {
            const source = protocolConfigs.find((config) => config.protocol === activeProtocol)
                || legacyProtocolConfig(formData);
            nextConfigs = [...protocolConfigs, {
                ...source,
                protocol,
                enabled: true,
            }];
        }

        const nextActiveProtocol = nextConfigs.find((config) => config.enabled && config.protocol === activeProtocol)?.protocol
            || nextConfigs.find((config) => config.enabled)?.protocol
            || activeProtocol;
        const nextActiveConfig = nextConfigs.find((config) => config.protocol === nextActiveProtocol)
            || nextConfigs[0];
        setProtocolConfigs(nextConfigs);
        setActiveProtocol(nextActiveProtocol);
        setFormData((current) => ({
            ...current,
            ...nextActiveConfig,
            protocol: nextActiveConfig.protocol,
            protocols: nextConfigs,
        }));
        setModelMappings(
            Object.entries(nextActiveConfig.model_mapping || {}).map(([from, to]) => ({ from, to })),
        );
        setUseOpenAIForModelDiscovery(false);
    };

    const updateModelConfigs = (next: ProviderModelConfig[]) => {
        setModelConfigs(next);
        setFormData((current) => ({
            ...current,
            model_configs: next,
            available_models: next
                .map((model) => model.id.trim())
                .filter(Boolean)
                .join(', '),
        }));
    };

    const updateModelConfig = (index: number, updates: Partial<ProviderModelConfig>) => {
        updateModelConfigs(modelConfigs.map((model, currentIndex) => (
            currentIndex === index ? { ...model, ...updates } : model
        )));
    };

    const removeModelConfig = (index: number) => {
        updateModelConfigs(modelConfigs.filter((_, currentIndex) => currentIndex !== index));
    };

    const addModelConfig = () => {
        updateModelConfigs([...modelConfigs, { id: '' }]);
        setExpandedModels((current) => new Set(current).add(`-${modelConfigs.length}`));
    };

    const toggleModelReasoning = (index: number, effort: string, checked: boolean) => {
        const current = modelConfigs[index]?.reasoning_efforts || [];
        const next = checked
            ? Array.from(new Set([...current, effort]))
            : current.filter((item) => item !== effort);
        updateModelConfig(index, { reasoning_efforts: next });
    };

    const addMapping = () => {
        if (newMappingFrom.trim() && newMappingTo.trim()) {
            const nextMappings = [...modelMappings, { from: newMappingFrom.trim(), to: newMappingTo.trim() }];
            setModelMappings(nextMappings);
            updateProtocolField('model_mapping', Object.fromEntries(
                nextMappings.map((mapping) => [mapping.from, mapping.to]),
            ));
            setNewMappingFrom('');
            setNewMappingTo('');
        }
    };

    const removeMapping = (index: number) => {
        const nextMappings = modelMappings.filter((_, i) => i !== index);
        setModelMappings(nextMappings);
        updateProtocolField('model_mapping', Object.fromEntries(
            nextMappings.map((mapping) => [mapping.from, mapping.to]),
        ));
    };

    const handleConfirm = () => {
        if (!formData.name.trim()) {
            showToast(t('providers.form.name_required'), 'error');
            return;
        }
        if (formData.provider_id && !/^[a-zA-Z][a-zA-Z0-9]{0,9}$/.test(formData.provider_id)) {
            showToast(t('providers.form.provider_id_invalid', 'Provider ID 格式错误（1-10位字母数字，首字符必须为字母）'), 'error');
            return;
        }
        const normalizedModels = modelConfigs
            .map((model) => ({
                ...model,
                id: model.id.trim(),
                ...(model.display_name?.trim() ? { display_name: model.display_name.trim() } : {}),
                ...(model.alias?.trim() ? { alias: model.alias.trim() } : {}),
            }))
            .filter((model) => model.id);
        const normalizedProtocolConfigs = protocolConfigs.map((config) => ({
            ...config,
            base_url: config.base_url.trim(),
            api_key: config.api_key.trim(),
            model_prefixes: config.model_prefixes.map((prefix) => prefix.trim()).filter(Boolean),
            model_mapping: config.protocol === activeProtocol
                ? Object.fromEntries(modelMappings.map((mapping) => [mapping.from.trim(), mapping.to.trim()]).filter(([from]) => from))
                : config.model_mapping,
        }));
        const enabledProtocolConfigs = normalizedProtocolConfigs.filter((config) => config.enabled);
        if (enabledProtocolConfigs.length === 0) {
            showToast('至少保留一个启用协议', 'error');
            return;
        }
        for (const config of enabledProtocolConfigs) {
            if (!config.base_url) {
                showToast(`${config.protocol}：${t('providers.form.base_url_required')}`, 'error');
                return;
            }
            if (!config.api_key && !config.credential_id) {
                showToast(`${config.protocol}：${t('providers.form.api_key_required')}`, 'error');
                return;
            }
        }
        const activeConfig = enabledProtocolConfigs.find((config) => config.protocol === activeProtocol)
            || enabledProtocolConfigs[0];
        onConfirm(
            {
                ...formData,
                ...activeConfig,
                protocol: activeConfig.protocol,
                protocols: normalizedProtocolConfigs,
                model_configs: normalizedModels,
                available_models: normalizedModels.map((model) => model.id).join(', '),
            },
            modelMappings,
            normalizedModels,
        );
    };

    const handleFetchModels = async () => {
        if (!formData.base_url.trim()) {
            showToast(t('providers.form.base_url_required'), 'error');
            return;
        }

        setFetchingModels(true);
        try {
            const rawModels = await request<unknown[]>('fetch_provider_models', {
                request: {
                    provider: formData,
                    use_openai_protocol:
                        formData.protocol === 'anthropic_passthrough' && useOpenAIForModelDiscovery,
                },
            });
            const models = normalizeDiscoveredModels(rawModels);
            if (models.length === 0) {
                throw new Error(t('providers.form.fetch_models_empty', '供应商没有返回可用模型'));
            }

            setModelCatalogState(models);
            setModelCatalogQuery('');
            setSelectedCatalogModels(new Set());
            setShowModelPicker(true);
            if (formData.provider_id?.trim()) {
                setModelCatalog(
                    providerModelCatalogKey(formData.provider_id.trim()),
                    models.map((model) => ({
                        id: model.id,
                        ...(model.display_name ? { display_name: model.display_name } : {}),
                    })),
                );
            }
            if (formData.credential_id?.trim()) {
                setModelCatalog(
                    codexModelCatalogKey(formData.credential_id.trim()),
                    models.map((model) => ({
                        id: model.id,
                        ...(model.display_name ? { display_name: model.display_name } : {}),
                    })),
                );
            }
            showToast(
                t('providers.form.fetch_models_success', '已获取 {{count}} 个模型', {
                    count: models.length,
                }),
                'success'
            );
        } catch (error: unknown) {
            const message =
                error instanceof Error
                    ? error.message
                    : typeof error === 'string'
                        ? error
                        : String(error);
            showToast(message, 'error');
        } finally {
            setFetchingModels(false);
        }
    };

    const filteredCatalogModels = modelCatalog.filter((model) => {
        const query = modelCatalogQuery.trim().toLowerCase();
        if (!query) return true;
        return model.id.toLowerCase().includes(query)
            || (model.display_name || '').toLowerCase().includes(query);
    });
    const existingModelIds = new Set(modelConfigs.map((model) => model.id.trim()).filter(Boolean));
    const selectableCatalogModels = filteredCatalogModels.filter((model) => !existingModelIds.has(model.id));
    const allFilteredModelsSelected = selectableCatalogModels.length > 0
        && selectableCatalogModels.every((model) => selectedCatalogModels.has(model.id));

    const toggleCatalogModel = (modelId: string, checked: boolean) => {
        setSelectedCatalogModels((current) => {
            const next = new Set(current);
            if (checked) next.add(modelId);
            else next.delete(modelId);
            return next;
        });
    };

    const toggleAllCatalogModels = (checked: boolean) => {
        setSelectedCatalogModels((current) => {
            const next = new Set(current);
            for (const model of selectableCatalogModels) {
                if (checked) next.add(model.id);
                else next.delete(model.id);
            }
            return next;
        });
    };

    const applySelectedCatalogModels = () => {
        const selected = modelCatalog.filter((model) => selectedCatalogModels.has(model.id));
        if (selected.length === 0) return;
        updateModelConfigs([
            ...modelConfigs,
            ...selected
                .filter((model) => !existingModelIds.has(model.id))
                .map((model) => ({ ...model })),
        ]);
        setShowModelPicker(false);
        setSelectedCatalogModels(new Set());
        showToast(`已添加 ${selected.length} 个模型`, 'success');
    };

    return createPortal(
        <div className="modal modal-open z-[100]">
            <div data-tauri-drag-region className="fixed top-0 left-0 right-0 h-8 z-[110]" />
            <div className="modal-box relative max-w-3xl bg-white dark:bg-base-100 shadow-2xl rounded-2xl p-0 overflow-hidden transform transition-all animate-in fade-in zoom-in-95 duration-200 max-h-[90vh] flex flex-col">
                {/* Header */}
                <div className="flex items-center justify-between px-6 pt-6 pb-4 border-b border-gray-100 dark:border-base-200">
                    <h3 className="text-lg font-bold text-gray-900 dark:text-base-content">
                        {editingProvider ? t('providers.form.edit_title') : t('providers.form.add_title')}
                    </h3>
                    <button
                        className="btn btn-ghost btn-sm btn-circle text-gray-400 hover:text-gray-600 dark:hover:text-gray-300"
                        onClick={onCancel}
                    >
                        <X size={18} />
                    </button>
                </div>

                {/* Body - scrollable */}
                <div className="flex-1 overflow-y-auto px-6 py-4 space-y-4">
                    {/* Name + Provider ID */}
                    <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
                        <div>
                            <label className="block text-xs font-medium text-gray-600 dark:text-gray-400 mb-1">
                                {t('providers.form.name_label')} <span className="text-red-500">*</span>
                            </label>
                            <input
                                ref={nameInputRef}
                                type="text"
                                className="input input-sm input-bordered w-full bg-white dark:bg-base-200 text-sm"
                                value={formData.name}
                                onChange={e => updateField('name', e.target.value)}
                                placeholder={t('providers.form.name_placeholder')}
                                disabled={!!editingProvider}
                            />
                        </div>
                        <div>
                            <label className="block text-xs font-medium text-gray-600 dark:text-gray-400 mb-1">
                                {t('providers.form.provider_id_label')}
                            </label>
                            <input
                                type="text"
                                className="input input-sm input-bordered w-full bg-white dark:bg-base-200 text-sm font-mono"
                                value={formData.provider_id || ''}
                                onChange={e => {
                                    const v = e.target.value.replace(/[^a-zA-Z0-9]/g, '');
                                    updateField('provider_id', v || undefined);
                                }}
                                placeholder={t('providers.form.provider_id_placeholder')}
                                maxLength={10}
                            />
                            <p className="text-[10px] text-gray-400 dark:text-gray-500 mt-0.5">
                                {t('providers.form.provider_id_hint', '1-10位字母数字，首字符必须为字母')}
                            </p>
                        </div>
                    </div>

                    {/* Protocols enabled for this logical provider.  The fields below
                        edit the highlighted protocol while models stay shared. */}
                    <div className="rounded-xl border border-indigo-100 bg-indigo-50/60 p-3 dark:border-indigo-900/50 dark:bg-indigo-950/20">
                        <div className="mb-2 flex items-center justify-between gap-3">
                            <label className="text-xs font-semibold text-gray-700 dark:text-gray-200">
                                启用协议
                            </label>
                            <span className="text-[10px] text-gray-500 dark:text-gray-400">
                                当前配置：{PROTOCOL_OPTIONS.find((option) => option.value === activeProtocol)?.label || activeProtocol}
                            </span>
                        </div>
                        <div className="flex flex-wrap gap-x-4 gap-y-2">
                            {PROTOCOL_OPTIONS.map((option) => {
                                const config = protocolConfigs.find((item) => item.protocol === option.value);
                                const enabled = config?.enabled === true;
                                return (
                                    <label key={option.value} className="inline-flex cursor-pointer items-center gap-1.5 text-xs text-gray-700 dark:text-gray-200">
                                        <input
                                            type="checkbox"
                                            className="checkbox checkbox-xs checkbox-primary"
                                            checked={enabled}
                                            onChange={(event) => toggleProtocol(option.value, event.target.checked)}
                                        />
                                        {option.label}
                                    </label>
                                );
                            })}
                        </div>
                        <div className="mt-3 flex flex-wrap gap-1.5 border-t border-indigo-100 pt-2 dark:border-indigo-900/40">
                            {protocolConfigs.filter((config) => config.enabled).map((config) => {
                                const option = PROTOCOL_OPTIONS.find((item) => item.value === config.protocol);
                                return (
                                    <button
                                        key={config.protocol}
                                        type="button"
                                        className={`rounded-lg border px-2.5 py-1 text-[10px] font-medium transition-colors ${activeProtocol === config.protocol
                                            ? 'border-indigo-500 bg-indigo-600 text-white shadow-sm'
                                            : 'border-indigo-200 bg-white text-indigo-700 hover:border-indigo-400 dark:border-indigo-800 dark:bg-base-200 dark:text-indigo-200'}`}
                                        onClick={() => selectProtocol(config.protocol)}
                                    >
                                        {option?.label || config.protocol}
                                    </button>
                                );
                            })}
                        </div>
                    </div>

                    {/* Base URL */}
                    <div>
                        <label className="block text-xs font-medium text-gray-600 dark:text-gray-400 mb-1">
                            {t('providers.form.base_url_label')} <span className="text-red-500">*</span>
                        </label>
                        <input
                            type="text"
                            className="input input-sm input-bordered w-full bg-white dark:bg-base-200 text-sm font-mono"
                            value={formData.base_url}
                            onChange={e => updateField('base_url', e.target.value)}
                            placeholder={t('providers.form.base_url_placeholder')}
                        />
                    </div>

                    {/* API Key / OAuth auth-file */}
                    <div>
                        <label className="block text-xs font-medium text-gray-600 dark:text-gray-400 mb-1">
                            {t('providers.form.api_key_label')}
                            {!formData.credential_id && <span className="text-red-500"> *</span>}
                        </label>
                        <div className="relative">
                            <input
                                type={showApiKey ? 'text' : 'password'}
                                className="input input-sm input-bordered w-full bg-white dark:bg-base-200 text-xs font-mono pr-10"
                                value={formData.api_key}
                                onChange={e => updateField('api_key', e.target.value)}
                                placeholder={t('providers.form.api_key_placeholder')}
                                disabled={!!formData.credential_id}
                            />
                            <button
                                type="button"
                                className="absolute right-2 top-1/2 -translate-y-1/2 text-gray-400 hover:text-gray-600 dark:hover:text-gray-300"
                                onClick={() => setShowApiKey(!showApiKey)}
                                tabIndex={-1}
                            >
                                {showApiKey ? <EyeOff size={16} /> : <Eye size={16} />}
                            </button>
                        </div>
                        <div className="mt-2">
                            <label className="block text-[11px] font-medium text-gray-500 dark:text-gray-400 mb-1">
                                {t('providers.form.auth_file_label', 'Codex OAuth auth-file')}
                            </label>
                            <select
                                className="select select-sm select-bordered w-full bg-white dark:bg-base-200 text-xs"
                                value={formData.credential_id || ''}
                                onChange={e => {
                                    const credentialId = e.target.value || undefined;
                                    updateField('credential_id', credentialId);
                                    if (credentialId) updateField('api_key', '');
                                }}
                            >
                                <option value="">
                                    {t('providers.form.auth_file_api_key_option', 'Use API Key')}
                                </option>
                                {codexConnections.map(connection => (
                                    <option key={connection.credential.id} value={connection.credential.id}>
                                        {connection.identity.email ||
                                            connection.identity.display_name ||
                                            connection.credential.fingerprint}
                                    </option>
                                ))}
                            </select>
                            <p className="text-[10px] text-gray-400 dark:text-gray-500 mt-0.5">
                                {t(
                                    'providers.form.auth_file_hint',
                                    '选择后请求会使用登录生成的 Codex auth-file',
                                )}
                            </p>
                        </div>
                    </div>

                    {/* Dispatch + Priority + Timeout */}
                    <div className="grid grid-cols-3 gap-3">
                        <div>
                            <label className="block text-xs font-medium text-gray-600 dark:text-gray-400 mb-1">
                                {t('providers.form.dispatch_mode_label')}
                            </label>
                            <select
                                className="select select-sm select-bordered w-full bg-white dark:bg-base-200 text-sm"
                                value={formData.dispatch_mode}
                                onChange={e => updateField('dispatch_mode', e.target.value as ProviderDispatchMode)}
                            >
                                {DISPATCH_OPTIONS.map(opt => (
                                    <option key={opt.value} value={opt.value}>{opt.label}</option>
                                ))}
                            </select>
                        </div>
                        <div>
                            <label className="block text-xs font-medium text-gray-600 dark:text-gray-400 mb-1">
                                {t('providers.form.priority_label')}
                            </label>
                            <input
                                type="number"
                                className="input input-sm input-bordered w-full bg-white dark:bg-base-200 text-sm"
                                value={formData.priority}
                                onChange={e => updateField('priority', parseInt(e.target.value) || 0)}
                                min={0}
                                max={99}
                                placeholder={t('providers.form.priority_placeholder')}
                            />
                        </div>
                        <div>
                            <label className="block text-xs font-medium text-gray-600 dark:text-gray-400 mb-1">
                                {t('providers.form.timeout_label')}
                            </label>
                            <input
                                type="number"
                                className="input input-sm input-bordered w-full bg-white dark:bg-base-200 text-sm"
                                value={formData.request_timeout_secs ?? ''}
                                onChange={e => {
                                    const v = parseInt(e.target.value);
                                    updateField('request_timeout_secs', isNaN(v) ? undefined : v);
                                }}
                                min={5}
                                placeholder={t('providers.form.timeout_placeholder')}
                            />
                        </div>
                    </div>

                    {/* Model catalog and per-model management */}
                    <div className="rounded-xl border border-gray-200 bg-white p-3 shadow-sm dark:border-base-300 dark:bg-base-100 space-y-3">
                        <div className="flex items-start justify-between gap-3">
                            <div>
                                <div className="flex items-center gap-1.5 text-xs font-semibold text-gray-900 dark:text-gray-100">
                                    <SlidersHorizontal size={14} className="text-blue-500" />
                                    {t('providers.form.available_models_label')}
                                    <span className="rounded-full border border-gray-200 bg-gray-50 px-1.5 py-0.5 text-[10px] font-normal text-gray-700 dark:border-base-300 dark:bg-base-200 dark:text-gray-200">
                                        {modelConfigs.filter((model) => model.id.trim()).length}
                                    </span>
                                </div>
                                <p className="mt-0.5 text-[10px] text-gray-600 dark:text-gray-300">
                                    {t('providers.form.available_models_hint', '管理可用模型、显示别名和模型能力')}
                                </p>
                            </div>
                            <div className="flex shrink-0 items-center gap-2">
                                {formData.protocol === 'anthropic_passthrough' && (
                                    <label className="inline-flex cursor-pointer items-center gap-1.5 whitespace-nowrap text-[10px] text-gray-600 dark:text-gray-400">
                                        <input
                                            type="checkbox"
                                            className="checkbox checkbox-xs checkbox-primary"
                                            checked={useOpenAIForModelDiscovery}
                                            onChange={e => setUseOpenAIForModelDiscovery(e.target.checked)}
                                        />
                                        {t('providers.form.fetch_models_openai_protocol')}
                                    </label>
                                )}
                                <button
                                    type="button"
                                    className="inline-flex items-center gap-1 rounded-lg border border-blue-200 bg-white px-2 py-1 text-[10px] font-medium text-blue-600 transition-colors hover:border-blue-400 hover:bg-blue-50 disabled:cursor-not-allowed disabled:text-gray-400 dark:border-blue-900/50 dark:bg-base-200 dark:hover:bg-blue-900/20"
                                    onClick={handleFetchModels}
                                    disabled={fetchingModels}
                                >
                                    {fetchingModels ? <Loader2 size={12} className="animate-spin" /> : <RefreshCw size={12} />}
                                    {fetchingModels
                                        ? t('providers.form.fetch_models_running', '获取中...')
                                        : t('providers.form.fetch_models', '从服务端获取')}
                                </button>
                            </div>
                        </div>

                        <div className="space-y-2">
                            {modelConfigs.length === 0 && (
                                <div className="rounded-lg border border-dashed border-gray-300 bg-white px-3 py-4 text-center text-[11px] text-gray-600 dark:border-gray-700 dark:bg-base-100 dark:text-gray-300">
                                    尚未添加模型，可从服务端获取或手动添加
                                </div>
                            )}
                            {modelConfigs.map((model, index) => {
                                const modelKey = `${model.id}-${index}`;
                                const expanded = expandedModels.has(modelKey) || expandedModels.has(model.id);
                                return (
                                    <div key={modelKey} className="rounded-lg border border-gray-200 bg-white dark:border-base-300 dark:bg-base-100">
                                        <div className="flex items-center gap-2 p-2">
                                            <div className="min-w-0 flex-1 grid grid-cols-1 gap-1.5 sm:grid-cols-2">
                                                <input
                                                    type="text"
                                                    className="input input-xs input-bordered w-full bg-white font-mono text-[11px] dark:bg-base-100"
                                                    value={model.id}
                                                    aria-label="model-id"
                                                    onChange={(event) => updateModelConfig(index, { id: event.target.value })}
                                                    placeholder="model-id"
                                                />
                                                <input
                                                    type="text"
                                                    className="input input-xs input-bordered w-full bg-white text-[11px] dark:bg-base-100"
                                                    value={model.alias || ''}
                                                    aria-label="model-alias"
                                                    onChange={(event) => updateModelConfig(index, { alias: event.target.value || undefined })}
                                                    placeholder="显示别名（可选）"
                                                />
                                            </div>
                                            <button
                                                type="button"
                                                className="btn btn-ghost btn-xs h-7 min-h-0 w-7 p-0 text-gray-400 hover:text-blue-500"
                                                onClick={() => setExpandedModels((current) => {
                                                    const next = new Set(current);
                                                    if (next.has(modelKey)) next.delete(modelKey);
                                                    else next.add(modelKey);
                                                    return next;
                                                })}
                                                title="模型高级配置"
                                            >
                                                {expanded ? <ChevronUp size={14} /> : <ChevronDown size={14} />}
                                            </button>
                                            <button
                                                type="button"
                                                className="btn btn-ghost btn-xs h-7 min-h-0 w-7 p-0 text-gray-400 hover:text-red-500"
                                                onClick={() => removeModelConfig(index)}
                                                title="删除模型"
                                            >
                                                <Trash2 size={13} />
                                            </button>
                                        </div>
                                        {(model.display_name || expanded) && (
                                            <div className="border-t border-gray-100 px-2 pb-2 pt-1.5 dark:border-base-300">
                                                {model.display_name && model.display_name !== model.id && (
                                                    <div className="mb-2 truncate text-[10px] text-gray-600 dark:text-gray-300" title={model.display_name}>
                                                        API 显示名称：{model.display_name}
                                                    </div>
                                                )}
                                                {expanded && (
                                                    <div className="space-y-2">
                                                        <label className="flex cursor-pointer items-center gap-2 text-[10px] text-gray-600 dark:text-gray-300">
                                                            <input
                                                                type="checkbox"
                                                                className="checkbox checkbox-xs checkbox-primary"
                                                                checked={model.supports_images === true}
                                                                onChange={(event) => updateModelConfig(index, { supports_images: event.target.checked })}
                                                            />
                                                            <Image size={12} />
                                                            允许图片端点
                                                        </label>
                                                        <div>
                                                            <div className="mb-1 text-[10px] font-medium text-gray-500 dark:text-gray-400">允许的思考档位</div>
                                                            <div className="flex flex-wrap gap-x-3 gap-y-1">
                                                                {MODEL_REASONING_OPTIONS.map((option) => (
                                                                    <label key={option.value} className="inline-flex cursor-pointer items-center gap-1 text-[10px] text-gray-600 dark:text-gray-300">
                                                                        <input
                                                                            type="checkbox"
                                                                            className="checkbox checkbox-xs checkbox-primary"
                                                                            checked={(model.reasoning_efforts || []).includes(option.value)}
                                                                            onChange={(event) => toggleModelReasoning(index, option.value, event.target.checked)}
                                                                        />
                                                                        {option.label}
                                                                    </label>
                                                                ))}
                                                            </div>
                                                        </div>
                                                    </div>
                                                )}
                                            </div>
                                        )}
                                    </div>
                                );
                            })}
                        </div>
                        <button
                            type="button"
                            className="inline-flex items-center gap-1 rounded-lg border border-dashed border-gray-300 px-2.5 py-1.5 text-[10px] font-medium text-gray-500 transition-colors hover:border-blue-400 hover:text-blue-600 dark:border-gray-700"
                            onClick={addModelConfig}
                        >
                            <Plus size={12} />
                            添加模型
                        </button>
                    </div>

                    {/* Advanced section */}
                    <details className="collapse collapse-arrow border border-gray-200 dark:border-base-300 rounded-lg" open={showAdvanced} onToggle={e => setShowAdvanced((e.target as HTMLDetailsElement).open)}>
                        <summary className="collapse-title text-sm font-medium text-gray-700 dark:text-gray-300">
                            {t('providers.form.model_mapping_title')}
                        </summary>
                        <div className="collapse-content space-y-3 pt-0">
                            {/* Model prefixes */}
                            <div>
                                <label className="block text-xs text-gray-500 dark:text-gray-400 mb-1">
                                    {t('providers.form.prefixes_label')}
                                </label>
                                <ModelPrefixTags
                                    prefixes={formData.model_prefixes}
                                    onChange={prefixes => updateField('model_prefixes', prefixes)}
                                />
                            </div>

                            {/* Model mapping */}
                            <div className="space-y-2">
                                <div className="flex items-center justify-between">
                                    <span className="text-xs font-medium text-gray-500 dark:text-gray-400">
                                        {t('providers.form.custom_mapping_label', 'Custom Mapping')}
                                    </span>
                                    <button
                                        className="text-xs text-blue-500 hover:text-blue-600"
                                        onClick={() => setShowMappingForm(!showMappingForm)}
                                    >
                                        {t('providers.form.model_mapping_add')}
                                    </button>
                                </div>
                                {showMappingForm && (
                                    <div className="flex gap-2">
                                        <input
                                            type="text"
                                            className="input input-xs input-bordered flex-1 bg-white dark:bg-base-200"
                                            placeholder={t('providers.form.model_mapping_from')}
                                            value={newMappingFrom}
                                            onChange={e => setNewMappingFrom(e.target.value)}
                                            onKeyDown={e => e.key === 'Enter' && addMapping()}
                                        />
                                        <input
                                            type="text"
                                            className="input input-xs input-bordered flex-1 bg-white dark:bg-base-200"
                                            placeholder={t('providers.form.model_mapping_to')}
                                            value={newMappingTo}
                                            onChange={e => setNewMappingTo(e.target.value)}
                                            onKeyDown={e => e.key === 'Enter' && addMapping()}
                                        />
                                        <button
                                            className="px-3 py-1.5 bg-blue-500 text-white text-xs font-medium rounded-lg hover:bg-blue-600 transition-all"
                                            onClick={addMapping}
                                        >
                                            OK
                                        </button>
                                    </div>
                                )}
                                {modelMappings.length > 0 && (
                                    <div className="space-y-1 max-h-28 overflow-y-auto">
                                        {modelMappings.map((m, i) => (
                                            <div
                                                key={i}
                                                className="flex items-center gap-2 text-xs bg-gray-50 dark:bg-base-300 px-2 py-1.5 rounded-lg"
                                            >
                                                <code className="font-mono">{m.from}</code>
                                                <span className="text-gray-400">→</span>
                                                <code className="font-mono">{m.to}</code>
                                                <button
                                                    className="ml-auto text-red-400 hover:text-red-500"
                                                    onClick={() => removeMapping(i)}
                                                >
                                                    <X size={12} />
                                                </button>
                                            </div>
                                        ))}
                                    </div>
                                )}
                                {modelMappings.length === 0 && !showMappingForm && (
                                    <div className="text-xs text-gray-400 text-center py-2">
                                        {t('providers.form.model_mapping_empty', 'No custom mappings')}
                                    </div>
                                )}
                            </div>
                        </div>
                    </details>
                </div>

                {showModelPicker && (
                    <div className="absolute inset-0 z-30 flex min-h-0 flex-col bg-white dark:bg-base-100">
                        <div className="flex items-center justify-between border-b border-gray-100 px-5 py-4 dark:border-base-200">
                            <div>
                                <h4 className="text-sm font-semibold text-gray-900 dark:text-base-content">从端点选择模型</h4>
                                <p className="mt-0.5 text-[10px] text-gray-500 dark:text-gray-400">已添加的模型不会被重复覆盖，可选择新模型后应用</p>
                            </div>
                            <button
                                type="button"
                                className="btn btn-ghost btn-sm btn-circle text-gray-400 hover:text-gray-700 dark:hover:text-gray-200"
                                onClick={() => setShowModelPicker(false)}
                                title="关闭"
                            >
                                <X size={16} />
                            </button>
                        </div>
                        <div className="flex min-h-0 flex-1 flex-col p-4">
                            <div className="flex items-center gap-2">
                                <div className="relative flex-1">
                                    <Search size={13} className="pointer-events-none absolute left-2.5 top-1/2 -translate-y-1/2 text-gray-400" />
                                    <input
                                        type="search"
                                        className="input input-sm input-bordered w-full bg-white pl-8 text-xs dark:bg-base-200"
                                        value={modelCatalogQuery}
                                        onChange={(event) => setModelCatalogQuery(event.target.value)}
                                        placeholder="搜索模型"
                                        aria-label="搜索模型"
                                        autoFocus
                                    />
                                </div>
                                <button
                                    type="button"
                                    className="btn btn-sm btn-ghost gap-1 text-xs"
                                    onClick={handleFetchModels}
                                    disabled={fetchingModels}
                                >
                                    <RefreshCw size={13} className={fetchingModels ? 'animate-spin' : ''} />
                                    重新加载
                                </button>
                            </div>
                            <div className="mt-3 flex items-center justify-between rounded-lg border border-gray-200 bg-white px-3 py-2 text-[10px] text-gray-700 dark:border-base-300 dark:bg-base-100 dark:text-gray-200">
                                <label className="inline-flex cursor-pointer items-center gap-2">
                                    <input
                                        type="checkbox"
                                        className="checkbox checkbox-xs checkbox-primary"
                                        checked={allFilteredModelsSelected}
                                        onChange={(event) => toggleAllCatalogModels(event.target.checked)}
                                        disabled={selectableCatalogModels.length === 0}
                                    />
                                    全选当前结果
                                </label>
                                <span>{selectedCatalogModels.size} / {modelCatalog.length}</span>
                            </div>
                            <div className="mt-2 min-h-0 flex-1 overflow-y-auto rounded-lg border border-gray-200 bg-white dark:border-base-300 dark:bg-base-100">
                                {filteredCatalogModels.length === 0 ? (
                                    <div className="p-8 text-center text-xs text-gray-600 dark:text-gray-300">没有匹配的模型</div>
                                ) : filteredCatalogModels.map((model) => {
                                    const added = existingModelIds.has(model.id);
                                    return (
                                        <label
                                            key={model.id}
                                            className={`flex cursor-pointer items-center gap-3 border-b border-gray-100 px-3 py-2.5 last:border-b-0 dark:border-base-300 ${added ? 'cursor-default opacity-60' : 'hover:bg-blue-50 dark:hover:bg-blue-900/10'}`}
                                        >
                                            <input
                                                type="checkbox"
                                                className="checkbox checkbox-xs checkbox-primary"
                                                checked={added || selectedCatalogModels.has(model.id)}
                                                disabled={added}
                                                onChange={(event) => toggleCatalogModel(model.id, event.target.checked)}
                                            />
                                            <div className="min-w-0 flex-1">
                                                <div className="truncate font-mono text-[11px] text-gray-800 dark:text-gray-200">{model.id}</div>
                                                {model.display_name && model.display_name !== model.id && (
                                                    <div className="truncate text-[10px] text-gray-600 dark:text-gray-300">{model.display_name}</div>
                                                )}
                                            </div>
                                            {added && <span className="rounded-full bg-gray-100 px-2 py-0.5 text-[9px] text-gray-500 dark:bg-base-300 dark:text-gray-400">已添加</span>}
                                        </label>
                                    );
                                })}
                            </div>
                        </div>
                        <div className="flex gap-2 border-t border-gray-100 px-5 py-3 dark:border-base-200">
                            <button
                                type="button"
                                className="btn btn-sm flex-1 bg-gray-100 text-gray-700 hover:bg-gray-200 dark:border-0 dark:bg-base-200 dark:text-gray-300"
                                onClick={() => setShowModelPicker(false)}
                            >
                                关闭
                            </button>
                            <button
                                type="button"
                                className="btn btn-sm flex-1 bg-blue-600 text-white hover:bg-blue-700 disabled:bg-gray-300 dark:disabled:bg-base-300"
                                onClick={applySelectedCatalogModels}
                                disabled={selectedCatalogModels.size === 0}
                            >
                                应用 ({selectedCatalogModels.size})
                            </button>
                        </div>
                    </div>
                )}

                {/* Footer */}
                <div className="flex gap-3 px-6 py-4 border-t border-gray-100 dark:border-base-200">
                    <button
                        className="flex-1 px-4 py-2.5 bg-gray-100 dark:bg-base-200 text-gray-700 dark:text-gray-300 font-medium rounded-xl hover:bg-gray-200 dark:hover:bg-base-300 transition-colors"
                        onClick={onCancel}
                    >
                        {t('providers.form.cancel')}
                    </button>
                    <button
                        className="flex-1 px-4 py-2.5 bg-blue-500 hover:bg-blue-600 text-white font-medium rounded-xl shadow-md transition-all"
                        onClick={handleConfirm}
                    >
                        {t('providers.form.confirm')}
                    </button>
                </div>
            </div>
            <div className="modal-backdrop bg-black/40 backdrop-blur-sm fixed inset-0 z-[-1]" onClick={onCancel}></div>
        </div>,
        document.body
    );
}

/* ─── Model Prefix Tags ─── */

function ModelPrefixTags({ prefixes, onChange }: { prefixes: string[]; onChange: (prefixes: string[]) => void }) {
    const [inputValue, setInputValue] = useState('');

    const addTag = () => {
        const trimmed = inputValue.trim();
        if (trimmed && !prefixes.includes(trimmed)) {
            onChange([...prefixes, trimmed]);
        }
        setInputValue('');
    };

    const removeTag = (index: number) => {
        onChange(prefixes.filter((_, i) => i !== index));
    };

    return (
        <div className="space-y-2">
            <div className="flex flex-wrap gap-1.5">
                {prefixes.map((prefix, i) => (
                    <span
                        key={i}
                        className="inline-flex items-center gap-1 px-2 py-0.5 rounded-md text-xs bg-gray-100 dark:bg-base-300 text-gray-700 dark:text-gray-300 border border-gray-200 dark:border-base-200"
                    >
                        {prefix}
                        <button className="text-gray-400 hover:text-red-500" onClick={() => removeTag(i)}>
                            <X size={10} />
                        </button>
                    </span>
                ))}
            </div>
            <div className="flex gap-2">
                <input
                    type="text"
                    className="input input-xs input-bordered flex-1 bg-white dark:bg-base-200"
                    placeholder="Add prefix..."
                    value={inputValue}
                    onChange={e => setInputValue(e.target.value)}
                    onKeyDown={e => { if (e.key === 'Enter') { e.preventDefault(); addTag(); } }}
                />
                <button className="px-3 py-1.5 bg-blue-500 text-white text-xs font-medium rounded-lg hover:bg-blue-600 transition-all" onClick={addTag}>+</button>
            </div>
        </div>
    );
}

/* ─── Main Page ─── */

function ProviderManager() {
    const { t } = useTranslation();
    const { config, saveConfig } = useConfigStore();

    const [searchText, setSearchText] = useState('');
    const [protocolFilter, setProtocolFilter] = useState<string>('');
    const [isModalOpen, setIsModalOpen] = useState(false);
    const [editingProvider, setEditingProvider] = useState<UpstreamProvider | null>(null);
    const [editingGroupKey, setEditingGroupKey] = useState<string | null>(null);
    const [modalData, setModalData] = useState<UpstreamProvider>(createEmptyProvider());
    const [deleteTarget, setDeleteTarget] = useState<UpstreamProvider | null>(null);
    const [testResults, setTestResults] = useState<{
        provider: UpstreamProvider;
        providerName: string;
        protocol: ProviderProtocol;
        model: string;
        models: string[];
        results: ModelTestResult[];
        currentModel?: string;
        status: 'running' | 'completed';
    } | null>(null);
    const [testDialog, setTestDialog] = useState<ProviderTestDialogState | null>(null);
    const [testingProvider, setTestingProvider] = useState<string | null>(null);
    const [codexConnections, setCodexConnections] = useState<CodexConnection[]>([]);

    const rawProviders: UpstreamProvider[] = useMemo(() => {
        return config?.proxy?.providers ?? [];
    }, [config?.proxy?.providers]);
    const providerGroups = useMemo(() => groupProviders(rawProviders), [rawProviders]);
    const providers: UpstreamProvider[] = useMemo(
        () => providerGroups.map((group) => group.provider),
        [providerGroups],
    );

    const loadCodexConnections = useCallback(async (): Promise<void> => {
        try {
            const connections = await request<CodexConnection[]>('list_account_connections');
            setCodexConnections(Array.isArray(connections) ? connections : []);
        } catch (error: unknown) {
            console.warn('Unable to load Codex auth-files', error);
        }
    }, []);

    useEffect(() => {
        void loadCodexConnections();
    }, [loadCodexConnections]);

    // Check for legacy z.ai config to migrate
    const hasLegacyZai = useMemo(() => {
        return config?.proxy?.zai?.enabled && (providers.length === 0);
    }, [config?.proxy?.zai?.enabled, providers.length]);

    const migrateZaiConfig = useCallback(() => {
        const zai = config?.proxy?.zai;
        if (!zai || !config) return;

        const newProvider: UpstreamProvider = {
            name: 'z.ai',
            enabled: zai.enabled,
            base_url: zai.base_url,
            api_key: zai.api_key,
            protocol: 'anthropic_passthrough',
            dispatch_mode: zai.dispatch_mode === 'off' ? 'pooled' : zai.dispatch_mode as ProviderDispatchMode,
            priority: 0,
            model_prefixes: [],
            model_mapping: zai.model_mapping || {},
        };

        const newConfig = {
            ...config,
            proxy: {
                ...config.proxy,
                providers: [newProvider],
            },
        };
        saveConfig(newConfig);
        showToast(t('providers.migrate_success'), 'success');
    }, [config, saveConfig, t]);

    // Filtered providers
    const filteredProviders = useMemo(() => {
        let result = providers;
        if (searchText) {
            const lower = searchText.toLowerCase();
            result = result.filter((p) => p.name.toLowerCase().includes(lower)
                || (p.provider_id || '').toLowerCase().includes(lower)
                || (p.provider_id_aliases || []).some((id) => id.toLowerCase().includes(lower)));
        }
        if (protocolFilter) {
            result = result.filter((p) => p.protocol === protocolFilter
                || (p.protocols || []).some((protocol) => protocol.enabled && protocol.protocol === protocolFilter));
        }
        return result;
    }, [providers, searchText, protocolFilter]);

    const handleAdd = () => {
        setEditingProvider(null);
        setEditingGroupKey(null);
        setModalData(createEmptyProvider());
        setIsModalOpen(true);
    };

    const handleEdit = (provider: UpstreamProvider) => {
        const group = providerGroups.find((item) => item.provider.name === provider.name);
        setEditingProvider(provider);
        setEditingGroupKey(group?.key || null);
        setModalData({ ...provider });
        setIsModalOpen(true);
    };

    const handleDeleteConfirm = async () => {
        if (!deleteTarget || !config) return;
        const group = providerGroups.find((item) => item.provider.name === deleteTarget.name);
        const memberKeys = new Set((group?.members || [deleteTarget]).map((provider) => (
            `${provider.name}\u0000${provider.provider_id || ''}`
        )));
        const newProviders = rawProviders.filter((provider) => !memberKeys.has(
            `${provider.name}\u0000${provider.provider_id || ''}`,
        ));
        const newConfig = {
            ...config,
            proxy: {
                ...config.proxy,
                providers: newProviders,
            },
        };
        await saveConfig(newConfig);
        showToast(t('providers.delete.success'), 'success');
        setDeleteTarget(null);
    };

    const handleToggleEnabled = async (provider: UpstreamProvider) => {
        if (!config) return;
        const group = providerGroups.find((item) => item.provider.name === provider.name);
        const memberKeys = new Set((group?.members || [provider]).map((item) => (
            `${item.name}\u0000${item.provider_id || ''}`
        )));
        const newProviders = rawProviders.map((item) => memberKeys.has(
            `${item.name}\u0000${item.provider_id || ''}`
        ) ? { ...item, enabled: !provider.enabled } : item);
        const newConfig = {
            ...config,
            proxy: {
                ...config.proxy,
                providers: newProviders,
            },
        };
        await saveConfig(newConfig);
    };

    const openTestDialog = useCallback((provider: UpstreamProvider) => {
        const protocols = enabledTestProtocols(provider);
        const models = testModelOptions(provider);
        if (protocols.length === 0) {
            showToast('至少配置一个启用协议后才能测试', 'warning');
            return;
        }
        if (models.length === 0) {
            showToast(t('providers.test.no_models'), 'warning');
            return;
        }
        setTestResults(null);
        setTestDialog({
            provider,
            protocol: protocols[0].protocol,
            model: ALL_TEST_MODELS,
        });
    }, [t]);

    const handleTestProvider = useCallback(async ({
        provider,
        protocol,
        model,
    }: ProviderTestDialogState) => {
        const modelsToTest = model === ALL_TEST_MODELS
            ? testModelOptions(provider).map((option) => option.value)
            : [model];
        if (modelsToTest.length === 0) {
            showToast(t('providers.test.no_models'), 'warning');
            return;
        }

        setTestingProvider(provider.name);
        setTestResults({
            provider,
            providerName: provider.name,
            protocol,
            model,
            models: modelsToTest,
            currentModel: modelsToTest[0],
            results: [],
            status: 'running',
        });

        try {
            for (const [index, modelId] of modelsToTest.entries()) {
                let results: ModelTestResult[];
                try {
                    const data = await request<{ results: ModelTestResult[] }>('test_provider_models', {
                        request: {
                            provider,
                            protocol,
                            model: modelId,
                        },
                    });
                    results = data.results.length > 0
                        ? data.results
                        : [{
                            model: modelId,
                            protocol,
                            success: false,
                            latency_ms: 0,
                            error: '未返回测试结果',
                        }];
                } catch (error: unknown) {
                    const message = error instanceof Error
                        ? error.message
                        : typeof error === 'object' && error !== null && 'message' in error
                            ? String((error as { message?: unknown }).message)
                            : String(error);
                    results = [{
                        model: modelId,
                        protocol,
                        success: false,
                        latency_ms: 0,
                        error: message,
                    }];
                }

                setTestResults((current) => current ? {
                    ...current,
                    results: [...current.results, ...results],
                    currentModel: modelsToTest[index + 1],
                } : current);
            }
        } finally {
            setTestingProvider(null);
            setTestResults((current) => current ? {
                ...current,
                currentModel: undefined,
                status: 'completed',
            } : current);
        }
    }, [t]);

    const handleModalConfirm = async (
        values: UpstreamProvider,
        mappings: { from: string; to: string }[],
        modelConfigs: ProviderModelConfig[],
    ) => {
        if (!config) return;

        // Build model_mapping
        const model_mapping: Record<string, string> = {};
        for (const m of mappings) {
            if (m.from.trim()) {
                model_mapping[m.from.trim()] = m.to.trim();
            }
        }

        // Ensure optional fields are explicitly set (avoid undefined which gets stripped by JSON.stringify)
        const updated: UpstreamProvider = {
            ...values,
            provider_id: values.provider_id || '',
            provider_group: values.provider_group || logicalProviderName(values.name).toLowerCase(),
            provider_id_aliases: Array.from(new Set(values.provider_id_aliases || []))
                .filter((id) => id && id !== values.provider_id),
            available_models: modelConfigs.map((model) => model.id.trim()).filter(Boolean).join(', '),
            model_configs: modelConfigs,
            model_mapping,
        };

        const editingGroup = editingGroupKey
            ? providerGroups.find((group) => group.key === editingGroupKey)
            : undefined;
        const editingMemberKeys = new Set((editingGroup?.members || []).map((provider) => (
            `${provider.name}\u0000${provider.provider_id || ''}`
        )));
        const otherProviders = providers.filter((provider) => !editingGroup || provider.name !== editingGroup.provider.name);

        // Check for duplicate name (only for new providers)
        if (!editingProvider && otherProviders.some((p) => p.name.toLowerCase() === updated.name.toLowerCase())) {
            showToast('Provider name already exists', 'error');
            return;
        }

        const newProviders: UpstreamProvider[] = editingProvider
            ? [...rawProviders.filter((provider) => !editingMemberKeys.has(
                `${provider.name}\u0000${provider.provider_id || ''}`,
            )), updated]
            : [...rawProviders, updated];

        // Check provider_id uniqueness
        const updatedRouteIds = new Set([
            ...(updated.provider_id ? [updated.provider_id] : []),
            ...(updated.provider_id_aliases || []),
        ]);
        const duplicate = otherProviders.some((provider) => {
            const existingRouteIds = new Set([
                ...(provider.provider_id ? [provider.provider_id] : []),
                ...(provider.provider_id_aliases || []),
            ]);
            return Array.from(updatedRouteIds).some((id) => existingRouteIds.has(id));
        });
        if (duplicate) {
            showToast('Provider ID already exists', 'error');
            return;
        }

        const newConfig = {
            ...config,
            proxy: {
                ...config.proxy,
                providers: newProviders,
            },
        };
        await saveConfig(newConfig);
        showToast(t('providers.form.save_success'), 'success');
        setIsModalOpen(false);
    };

    const testDialogProtocols = testDialog ? enabledTestProtocols(testDialog.provider) : [];
    const testDialogModels = testDialog ? testModelOptions(testDialog.provider) : [];
    const testDialogModelCount = testDialog?.model === ALL_TEST_MODELS ? testDialogModels.length : 1;
    const isTestRunning = testDialog ? testingProvider === testDialog.provider.name : false;

    return (
        <div className="h-full flex flex-col p-5 gap-4 max-w-7xl mx-auto w-full">
            {/* Header */}
            <div className="flex items-center justify-between">
                <div className="flex items-center gap-3">
                    <Server size={22} className="text-indigo-500" />
                    <div>
                        <h1 className="text-lg font-bold text-gray-900 dark:text-white">
                            {t('providers.title')}
                        </h1>
                        <p className="text-xs text-gray-500 dark:text-gray-400">
                            {t('providers.subtitle')}
                        </p>
                    </div>
                </div>
                <button
                    className="px-4 py-2 bg-blue-500 text-white text-sm font-medium rounded-xl shadow-md hover:bg-blue-600 transition-all flex items-center gap-1.5"
                    onClick={handleAdd}
                >
                    <Plus size={14} />
                    {t('providers.add_button')}
                </button>
            </div>

            {/* Migration banner */}
            {hasLegacyZai && (
                <div className="p-4 bg-amber-50 dark:bg-amber-900/20 border border-amber-200 dark:border-amber-800 rounded-xl flex items-start gap-3">
                    <AlertCircle size={18} className="text-amber-500 mt-0.5 flex-shrink-0" />
                    <div className="flex-1">
                        <p className="text-sm font-medium text-amber-800 dark:text-amber-200">
                            {t('providers.migrate_zai_title')}
                        </p>
                        <p className="text-xs text-amber-600 dark:text-amber-300 mt-0.5">
                            {t('providers.migrate_zai_description')}
                        </p>
                        <div className="mt-3 flex gap-3">
                            <button
                                className="px-4 py-2 bg-amber-500 text-white text-sm font-medium rounded-xl shadow-md hover:bg-amber-600 transition-all"
                                onClick={migrateZaiConfig}
                            >
                                {t('providers.migrate_button')}
                            </button>
                            <button
                                className="px-4 py-2 bg-gray-100 dark:bg-base-200 text-gray-700 dark:text-gray-300 text-sm font-medium rounded-xl hover:bg-gray-200 dark:hover:bg-base-300 transition-colors"
                                onClick={() => { }}
                            >
                                {t('providers.skip_button')}
                            </button>
                        </div>
                    </div>
                </div>
            )}

            {/* Search & Filter Bar */}
            <div className="bg-white dark:bg-base-100 rounded-xl shadow-sm border border-gray-100 dark:border-base-200 p-3">
                <div className="flex items-center gap-3">
                    <div className="relative">
                        <Search size={14} className="absolute left-2.5 top-1/2 -translate-y-1/2 text-gray-400" />
                        <input
                            type="text"
                            className="pl-9 pr-4 py-2 bg-white dark:bg-base-100 text-sm border border-gray-200 dark:border-base-300 rounded-lg focus:ring-2 focus:ring-blue-500 focus:outline-none w-56"
                            placeholder={t('providers.search_placeholder')}
                            value={searchText}
                            onChange={e => setSearchText(e.target.value)}
                        />
                        {searchText && (
                            <button
                                className="absolute right-2 top-1/2 -translate-y-1/2 text-gray-400 hover:text-gray-600"
                                onClick={() => setSearchText('')}
                            >
                                <X size={12} />
                            </button>
                        )}
                    </div>

                    {/* Protocol filter buttons */}
                    <div className="bg-gray-100 dark:bg-base-200 p-1 rounded-lg flex gap-0.5">
                        <button
                            className={`px-3 py-1 text-xs rounded-md transition-all ${!protocolFilter
                                    ? 'bg-white dark:bg-base-100 text-blue-600 shadow-sm font-medium'
                                    : 'text-gray-500 dark:text-gray-400 hover:text-gray-700'
                                }`}
                            onClick={() => setProtocolFilter('')}
                        >
                            {t('providers.filter_all')}
                        </button>
                        {PROTOCOL_OPTIONS.map(opt => (
                            <button
                                key={opt.value}
                                className={`px-3 py-1 text-xs rounded-md transition-all ${protocolFilter === opt.value
                                        ? 'bg-white dark:bg-base-100 text-blue-600 shadow-sm font-medium'
                                        : 'text-gray-500 dark:text-gray-400 hover:text-gray-700'
                                    }`}
                                onClick={() => setProtocolFilter(protocolFilter === opt.value ? '' : opt.value)}
                            >
                                {opt.label}
                            </button>
                        ))}
                    </div>

                    <span className="text-xs text-gray-400 ml-auto">
                        {t('providers.total', { count: filteredProviders.length })}
                    </span>
                </div>
            </div>

            {/* Table */}
            <div className="bg-white dark:bg-base-100 rounded-xl shadow-sm border border-gray-100 dark:border-base-200 flex-1 overflow-hidden flex flex-col">
                {providers.length === 0 ? (
                    <div className="flex flex-col items-center justify-center py-16 text-center">
                        <Server size={48} className="text-gray-300 dark:text-gray-600 mb-4" />
                        <p className="text-gray-500 dark:text-gray-400 text-sm">
                            {t('providers.empty_description')}
                        </p>
                        <button
                            className="px-4 py-2 bg-blue-500 text-white text-sm font-medium rounded-xl shadow-md hover:bg-blue-600 transition-all flex items-center gap-1.5 mt-3"
                            onClick={handleAdd}
                        >
                            <Plus size={14} />
                            {t('providers.empty_action')}
                        </button>
                    </div>
                ) : (
                    <div className="overflow-auto flex-1">
                        <table className="w-full">
                            <thead className="sticky top-0 z-10">
                                <tr className="border-b border-gray-100 dark:border-base-200 bg-gray-50 dark:bg-base-200">
                                    <th className="px-4 py-2.5 text-left text-xs font-medium text-gray-500 dark:text-gray-400 w-16">
                                        {t('providers.columns.status')}
                                    </th>
                                    <th className="px-4 py-2.5 text-left text-xs font-medium text-gray-500 dark:text-gray-400 w-36">
                                        {t('providers.columns.name')}
                                    </th>
                                    <th className="px-4 py-2.5 text-left text-xs font-medium text-gray-500 dark:text-gray-400 w-24">
                                        {t('providers.columns.provider_id', 'Route ID')}
                                    </th>
                                    <th className="px-4 py-2.5 text-left text-xs font-medium text-gray-500 dark:text-gray-400">
                                        {t('providers.columns.base_url')}
                                    </th>
                                    <th className="px-4 py-2.5 text-left text-xs font-medium text-gray-500 dark:text-gray-400 w-32">
                                        {t('providers.columns.protocol')}
                                    </th>
                                    <th className="px-4 py-2.5 text-left text-xs font-medium text-gray-500 dark:text-gray-400 w-28">
                                        {t('providers.columns.dispatch_mode')}
                                    </th>
                                    <th className="px-4 py-2.5 text-left text-xs font-medium text-gray-500 dark:text-gray-400 w-20">
                                        {t('providers.columns.priority')}
                                    </th>
                                    <th className="px-4 py-2.5 text-center text-xs font-medium text-gray-500 dark:text-gray-400 w-32">
                                        {t('providers.columns.actions')}
                                    </th>
                                </tr>
                            </thead>
                            <tbody>
                                {filteredProviders.length === 0 ? (
                                    <tr>
                                        <td colSpan={8} className="text-center py-8 text-gray-400 text-sm">
                                            {t('providers.no_results', 'No matching providers')}
                                        </td>
                                    </tr>
                                ) : (
                                    filteredProviders.map((provider) => {
                                        const displayProtocols = provider.protocols?.filter((protocol) => protocol.enabled).map((protocol) => protocol.protocol)
                                            || [provider.protocol];
                                        return (
                                        <tr
                                            key={provider.provider_group || provider.name}
                                            className="border-b border-gray-50 dark:border-base-300 hover:bg-gray-50 dark:hover:bg-gray-700/30 transition-colors"
                                        >
                                            <td className="px-4 py-3">
                                                <input
                                                    type="checkbox"
                                                    className="toggle toggle-sm toggle-blue"
                                                    checked={provider.enabled}
                                                    onChange={() => handleToggleEnabled(provider)}
                                                />
                                            </td>
                                            <td className="px-4 py-3">
                                                <span className="font-medium text-sm">{provider.name}</span>
                                            </td>
                                            <td className="px-4 py-3">
                                                {provider.provider_id ? (
                                                    <span className="inline-flex items-center px-1.5 py-0.5 rounded text-[10px] font-mono font-bold bg-blue-100 dark:bg-blue-900/30 text-blue-700 dark:text-blue-300 border border-blue-200 dark:border-blue-800">
                                                        {provider.provider_id}
                                                    </span>
                                                ) : (
                                                    <span className="text-[10px] text-gray-300 dark:text-gray-600">—</span>
                                                )}
                                            </td>
                                            <td className="px-4 py-3">
                                                <code className="text-xs text-gray-500 dark:text-gray-400 font-mono truncate block max-w-xs">
                                                    {provider.base_url}
                                                </code>
                                            </td>
                                            <td className="px-4 py-3">
                                                <div className="flex flex-wrap gap-1">
                                                    {displayProtocols.map((protocol) => (
                                                        <span key={protocol} className={`inline-flex items-center px-2 py-0.5 rounded-md text-[10px] font-bold shadow-sm border ${PROTOCOL_COLORS[protocol]}`}>
                                                            {t(`providers.protocols.${protocol}`)}
                                                        </span>
                                                    ))}
                                                </div>
                                            </td>
                                            <td className="px-4 py-3">
                                                <span className="text-xs text-gray-600 dark:text-gray-400">
                                                    {t(`providers.dispatch_modes.${provider.dispatch_mode}`)}
                                                </span>
                                            </td>
                                            <td className="px-4 py-3">
                                                <span className="text-xs text-gray-600 dark:text-gray-400">
                                                    {provider.priority}
                                                </span>
                                            </td>
                                            <td className="px-4 py-3">
                                                <div className="flex items-center justify-center gap-1">
                                                    <button
                                                        className="btn btn-ghost btn-xs gap-1 text-xs text-purple-500 hover:text-purple-600 hover:bg-purple-50 dark:hover:bg-purple-900/20"
                                                        onClick={() => openTestDialog(provider)}
                                                        disabled={testingProvider === provider.name}
                                                    >
                                                        {testingProvider === provider.name ? (
                                                            <Loader2 size={12} className="animate-spin" />
                                                        ) : (
                                                            <FlaskConical size={12} />
                                                        )}
                                                        {testingProvider === provider.name ? t('providers.test.running') : t('providers.test_button')}
                                                    </button>
                                                    <button
                                                        className="btn btn-ghost btn-xs gap-1 text-xs text-blue-500 hover:text-blue-600 hover:bg-blue-50 dark:hover:bg-blue-900/20"
                                                        onClick={() => handleEdit(provider)}
                                                    >
                                                        <Edit2 size={12} />
                                                        {t('providers.edit_button')}
                                                    </button>
                                                    <button
                                                        className="btn btn-ghost btn-xs gap-1 text-xs text-red-500 hover:text-red-600 hover:bg-red-50 dark:hover:bg-red-900/20"
                                                        onClick={() => setDeleteTarget(provider)}
                                                    >
                                                        <Trash2 size={12} />
                                                        {t('providers.delete_button')}
                                                    </button>
                                                </div>
                                            </td>
                                        </tr>
                                        );
                                    })
                                )}
                            </tbody>
                        </table>
                    </div>
                )}
            </div>

            {/* Add/Edit Modal */}
            <ProviderFormModal
                isOpen={isModalOpen}
                editingProvider={editingProvider}
                initialData={modalData}
                codexConnections={codexConnections}
                onConfirm={handleModalConfirm}
                onCancel={() => setIsModalOpen(false)}
            />

            {/* Delete Confirmation */}
            <ModalDialog
                isOpen={!!deleteTarget}
                title={t('providers.delete.title')}
                message={deleteTarget ? t('providers.delete.description', { name: deleteTarget.name }) : undefined}
                type="confirm"
                isDestructive
                onConfirm={handleDeleteConfirm}
                onCancel={() => setDeleteTarget(null)}
                confirmText={t('providers.delete.confirm')}
                cancelText={t('providers.delete.cancel')}
            />

            {/* Test Scope Modal */}
            {testDialog && createPortal(
                <div className="modal modal-open z-[100]">
                    <div className="modal-box relative flex max-h-[85vh] max-w-lg flex-col overflow-hidden rounded-2xl bg-white p-0 shadow-2xl dark:bg-[#0f172a]">
                        <div className="flex items-start justify-between gap-4 border-b border-gray-100 px-6 pb-4 pt-6 dark:border-indigo-400/20">
                            <div className="flex items-start gap-3">
                                <div className="rounded-xl bg-purple-50 p-2 text-purple-600 dark:bg-purple-900/20 dark:text-purple-300">
                                    <FlaskConical size={20} />
                                </div>
                                <div>
                                    <h3 className="text-lg font-bold text-gray-900 dark:text-base-content">
                                        {t('providers.test.scope_title', '选择测试范围')}
                                    </h3>
                                    <p className="mt-1 text-xs text-gray-500 dark:text-slate-300">
                                        {testDialog.provider.name} · {t('providers.test.scope_description', '按协议和模型进行测试，避免一次请求全部模型')}
                                    </p>
                                </div>
                            </div>
                            <button
                                className="btn btn-ghost btn-sm btn-circle text-gray-400 hover:text-gray-600 dark:hover:text-gray-300"
                                disabled={isTestRunning}
                                onClick={() => {
                                    if (!isTestRunning) {
                                        setTestDialog(null);
                                        setTestResults(null);
                                    }
                                }}
                                aria-label={t('common.close', '关闭')}
                            >
                                <X size={18} />
                            </button>
                        </div>

                        <div className="flex-1 space-y-4 overflow-y-auto px-6 py-5">
                            <div>
                                <label className="mb-1.5 block text-xs font-semibold text-gray-700 dark:text-gray-200">
                                    {t('providers.test.protocol_label', '测试协议')}
                                </label>
                                <select
                                    className="select select-sm h-10 w-full border-gray-300 bg-white text-sm dark:border-indigo-400/30 dark:bg-[#111c36] dark:text-slate-100"
                                    value={testDialog.protocol}
                                    disabled={isTestRunning}
                                    onChange={(event) => {
                                        setTestResults(null);
                                        setTestDialog((current) => current
                                            ? { ...current, protocol: event.target.value as ProviderProtocol }
                                            : current);
                                    }}
                                >
                                    {testDialogProtocols.map((protocol) => (
                                        <option key={protocol.protocol} value={protocol.protocol}>
                                            {PROTOCOL_OPTIONS.find((option) => option.value === protocol.protocol)?.label
                                                || protocol.protocol}
                                        </option>
                                    ))}
                                </select>
                                <p className="mt-1 truncate text-[10px] text-gray-400 dark:text-slate-300" title={testDialogProtocols.find((protocol) => protocol.protocol === testDialog.protocol)?.base_url}>
                                    {testDialogProtocols.find((protocol) => protocol.protocol === testDialog.protocol)?.base_url}
                                </p>
                            </div>

                            <div>
                                <label className="mb-1.5 block text-xs font-semibold text-gray-700 dark:text-gray-200">
                                    {t('providers.test.model_label', '测试模型')}
                                </label>
                                <GroupedSelect
                                    value={testDialog.model}
                                    disabled={isTestRunning}
                                    onChange={(model) => {
                                        setTestResults(null);
                                        setTestDialog((current) => current
                                            ? { ...current, model }
                                            : current);
                                    }}
                                    options={[
                                        {
                                            value: ALL_TEST_MODELS,
                                            label: t('providers.test.all_models', '全部模型'),
                                            group: t('providers.test.scope_group', '测试范围'),
                                        },
                                        ...testDialogModels,
                                    ]}
                                    placeholder={t('providers.test.model_placeholder', '选择模型')}
                                    className="w-full"
                                />
                            </div>

                            <div className="flex items-center justify-between rounded-xl border border-purple-100 bg-purple-50/70 px-3 py-2.5 text-xs dark:border-purple-900/40 dark:bg-purple-950/20">
                                <span className="text-purple-700 dark:text-purple-200">
                                    {t('providers.test.scope_summary', '本次将测试 {{count}} 个模型', { count: testDialogModelCount })}
                                </span>
                                <span className="font-mono text-[10px] text-purple-500 dark:text-purple-300">
                                    {testDialog.model === ALL_TEST_MODELS
                                        ? t('providers.test.all_models', '全部模型')
                                        : testDialog.model}
                                </span>
                            </div>

                            {testResults && (
                                <div className="overflow-hidden rounded-xl border border-slate-200 bg-white dark:border-indigo-400/25 dark:bg-[#0b1220]">
                                    <div className="flex items-center justify-between gap-3 border-b border-slate-200 px-3 py-2.5 dark:border-indigo-400/20">
                                        <div className="flex min-w-0 items-center gap-2 text-xs font-semibold text-slate-700 dark:text-slate-100">
                                            {isTestRunning ? (
                                                <Loader2 size={14} className="shrink-0 animate-spin text-purple-500" />
                                            ) : (
                                                <CheckCircle2 size={14} className="shrink-0 text-green-500" />
                                            )}
                                            <span>{isTestRunning ? '测试进行中' : '测试完成'}</span>
                                        </div>
                                        <span className="shrink-0 text-[10px] tabular-nums text-slate-600 dark:text-slate-300">
                                            {testResults.results.filter((result) => result.success).length}/
                                            {testResults.results.length} 成功 · {testResults.results.length}/
                                            {testResults.models.length}
                                        </span>
                                    </div>

                                    <div className="max-h-64 space-y-2 overflow-y-auto p-3">
                                        {testResults.results.map((result, index) => (
                                            <div
                                                key={`${result.model}-${result.protocol || ''}-${index}`}
                                                className="flex items-center gap-3 rounded-lg border px-3 py-2 text-sm text-slate-800 dark:text-slate-100"
                                                style={{
                                                    borderColor: result.success ? 'rgb(34 197 94 / 0.3)' : 'rgb(239 68 68 / 0.3)',
                                                    backgroundColor: result.success ? 'rgb(34 197 94 / 0.05)' : 'rgb(239 68 68 / 0.05)',
                                                }}
                                            >
                                                {result.success ? (
                                                    <CheckCircle2 size={16} className="shrink-0 text-green-500" />
                                                ) : (
                                                    <XCircle size={16} className="shrink-0 text-red-500" />
                                                )}
                                                <code className="min-w-0 flex-1 truncate font-mono text-xs">
                                                    {result.model}{result.protocol ? ` · ${result.protocol}` : ''}
                                                </code>
                                                <span className="whitespace-nowrap text-xs tabular-nums text-slate-600 dark:text-slate-300">
                                                    {(result.latency_ms || result.latencyMs || 0)}ms
                                                </span>
                                                {result.error && (
                                                    <span className="max-w-[220px] truncate text-xs text-red-600 dark:text-red-300" title={result.error}>
                                                        {result.error}
                                                    </span>
                                                )}
                                            </div>
                                        ))}
                                        {isTestRunning && testResults.results.length === 0 && (
                                            <div className="flex items-center gap-2 rounded-lg border border-dashed border-purple-200 bg-purple-50/50 px-3 py-4 text-xs text-purple-700 dark:border-indigo-400/30 dark:bg-indigo-950/40 dark:text-indigo-200">
                                                <Loader2 size={14} className="shrink-0 animate-spin" />
                                                正在测试：<code className="truncate font-mono">{testResults.currentModel}</code>
                                            </div>
                                        )}
                                    </div>

                                    <div className="border-t border-slate-200 px-3 py-2 text-[10px] text-slate-600 dark:border-indigo-400/20 dark:text-slate-300">
                                        {isTestRunning && testResults.currentModel ? (
                                            <span>
                                                正在测试：<code className="font-mono text-purple-600 dark:text-purple-300">{testResults.currentModel}</code>
                                            </span>
                                        ) : (
                                            '全部模型测试完成'
                                        )}
                                    </div>
                                </div>
                            )}
                        </div>

                        <div className="flex gap-3 border-t border-gray-100 px-6 py-4 dark:border-indigo-400/20">
                            <button
                                className="flex-1 rounded-xl bg-gray-100 px-4 py-2.5 font-medium text-gray-700 transition-colors hover:bg-gray-200 dark:border dark:border-indigo-400/20 dark:bg-indigo-950/50 dark:text-slate-200 dark:hover:bg-indigo-900/60"
                                disabled={isTestRunning}
                                onClick={() => {
                                    if (!isTestRunning) {
                                        setTestDialog(null);
                                        setTestResults(null);
                                    }
                                }}
                            >
                                {t('common.cancel', '取消')}
                            </button>
                            <button
                                className="flex-1 rounded-xl bg-purple-600 px-4 py-2.5 font-medium text-white shadow-md transition-colors hover:bg-purple-700 disabled:cursor-not-allowed disabled:opacity-50"
                                disabled={isTestRunning || testDialogModelCount === 0}
                                onClick={() => {
                                    void handleTestProvider(testDialog);
                                }}
                            >
                                <span className="inline-flex items-center justify-center gap-2">
                                    {isTestRunning ? (
                                        <Loader2 size={14} className="animate-spin" />
                                    ) : testResults?.status === 'completed' ? (
                                        <RotateCcw size={14} />
                                    ) : (
                                        <FlaskConical size={14} />
                                    )}
                                    {isTestRunning
                                        ? t('providers.test.running')
                                        : testResults?.status === 'completed'
                                            ? t('providers.test.retest')
                                            : t('providers.test.start', '开始测试')}
                                </span>
                            </button>
                        </div>
                    </div>
                    <div
                        className="modal-backdrop fixed inset-0 z-[-1] bg-black/40 backdrop-blur-sm"
                        onClick={() => {
                            if (!isTestRunning) {
                                setTestDialog(null);
                                setTestResults(null);
                            }
                        }}
                    ></div>
                </div>,
                document.body
            )}

        </div>
    );
}

export default ProviderManager;
