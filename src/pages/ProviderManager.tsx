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
} from 'lucide-react';
import { useConfigStore } from '../stores/useConfigStore';
import { showToast } from '../components/common/ToastContainer';
import ModalDialog from '../components/common/ModalDialog';
import { request } from '../utils/request';
import type {
    UpstreamProvider,
    ProviderProtocol,
    ProviderDispatchMode,
} from '../types/config';

interface ModelTestResult {
    model: string;
    success: boolean;
    latency_ms: number;
    latencyMs?: number;
    error?: string;
}

const PROTOCOL_OPTIONS: { value: ProviderProtocol; label: string }[] = [
    { value: 'anthropic_passthrough', label: 'Anthropic' },
    { value: 'openai_compatible', label: 'OpenAI' },
    { value: 'gemini_v1internal', label: 'Gemini' },
];

const DISPATCH_OPTIONS: { value: ProviderDispatchMode; label: string }[] = [
    { value: 'exclusive', label: 'Exclusive' },
    { value: 'pooled', label: 'Pooled' },
    { value: 'fallback', label: 'Fallback' },
];

const PROTOCOL_COLORS: Record<ProviderProtocol, string> = {
    anthropic_passthrough: 'bg-blue-100 dark:bg-blue-900/30 text-blue-700 dark:text-blue-300 border-blue-200 dark:border-blue-800',
    openai_compatible: 'bg-green-100 dark:bg-green-900/30 text-green-700 dark:text-green-300 border-green-200 dark:border-green-800',
    gemini_v1internal: 'bg-orange-100 dark:bg-orange-900/30 text-orange-700 dark:text-orange-300 border-orange-200 dark:border-orange-800',
};

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
    };
}

/* ─── Custom Modal Dialog for Add/Edit ─── */

interface ProviderFormModalProps {
    isOpen: boolean;
    editingProvider: UpstreamProvider | null;
    initialData: UpstreamProvider;
    onConfirm: (values: UpstreamProvider, mappings: { from: string; to: string }[]) => void;
    onCancel: () => void;
}

function ProviderFormModal({ isOpen, editingProvider, initialData, onConfirm, onCancel }: ProviderFormModalProps) {
    const { t } = useTranslation();
    const [formData, setFormData] = useState<UpstreamProvider>(initialData);
    const [modelMappings, setModelMappings] = useState<{ from: string; to: string }[]>([]);
    const [newMappingFrom, setNewMappingFrom] = useState('');
    const [newMappingTo, setNewMappingTo] = useState('');
    const [showMappingForm, setShowMappingForm] = useState(false);
    const [showAdvanced, setShowAdvanced] = useState(true);
    const [showApiKey, setShowApiKey] = useState(false);
    const nameInputRef = useRef<HTMLInputElement>(null);

    useEffect(() => {
        setFormData(initialData);
        setModelMappings(
            Object.entries(initialData.model_mapping || {}).map(([from, to]) => ({ from, to }))
        );
        setShowMappingForm(false);
        setNewMappingFrom('');
        setNewMappingTo('');
        setShowAdvanced(true);
        setShowApiKey(false);
    }, [initialData]);

    useEffect(() => {
        if (isOpen && nameInputRef.current) {
            setTimeout(() => nameInputRef.current?.focus(), 100);
        }
    }, [isOpen]);

    if (!isOpen) return null;

    const updateField = <K extends keyof UpstreamProvider>(field: K, value: UpstreamProvider[K]) => {
        setFormData(prev => ({ ...prev, [field]: value }));
    };

    const addMapping = () => {
        if (newMappingFrom.trim() && newMappingTo.trim()) {
            setModelMappings(prev => [...prev, { from: newMappingFrom.trim(), to: newMappingTo.trim() }]);
            setNewMappingFrom('');
            setNewMappingTo('');
        }
    };

    const removeMapping = (index: number) => {
        setModelMappings(prev => prev.filter((_, i) => i !== index));
    };

    const handleConfirm = () => {
        if (!formData.name.trim()) {
            showToast(t('providers.form.name_required'), 'error');
            return;
        }
        if (!formData.base_url.trim()) {
            showToast(t('providers.form.base_url_required'), 'error');
            return;
        }
        if (!formData.api_key.trim()) {
            showToast(t('providers.form.api_key_required'), 'error');
            return;
        }
        if (formData.provider_id && !/^[a-zA-Z][a-zA-Z0-9]{0,9}$/.test(formData.provider_id)) {
            showToast(t('providers.form.provider_id_invalid', 'Provider ID 格式错误（1-10位字母数字，首字符必须为字母）'), 'error');
            return;
        }
        onConfirm(formData, modelMappings);
    };

    return createPortal(
        <div className="modal modal-open z-[100]">
            <div data-tauri-drag-region className="fixed top-0 left-0 right-0 h-8 z-[110]" />
            <div className="modal-box relative max-w-lg bg-white dark:bg-base-100 shadow-2xl rounded-2xl p-0 overflow-hidden transform transition-all animate-in fade-in zoom-in-95 duration-200 max-h-[90vh] flex flex-col">
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
                    {/* Name + Provider ID + Protocol */}
                    <div className="grid grid-cols-3 gap-3">
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
                        <div>
                            <label className="block text-xs font-medium text-gray-600 dark:text-gray-400 mb-1">
                                {t('providers.form.protocol_label')}
                            </label>
                            <select
                                className="select select-sm select-bordered w-full bg-white dark:bg-base-200 text-sm"
                                value={formData.protocol}
                                onChange={e => updateField('protocol', e.target.value as ProviderProtocol)}
                            >
                                {PROTOCOL_OPTIONS.map(opt => (
                                    <option key={opt.value} value={opt.value}>{opt.label}</option>
                                ))}
                            </select>
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

                    {/* API Key */}
                    <div>
                        <label className="block text-xs font-medium text-gray-600 dark:text-gray-400 mb-1">
                            {t('providers.form.api_key_label')} <span className="text-red-500">*</span>
                        </label>
                        <div className="relative">
                            <input
                                type={showApiKey ? 'text' : 'password'}
                                className="input input-sm input-bordered w-full bg-white dark:bg-base-200 text-xs font-mono pr-10"
                                value={formData.api_key}
                                onChange={e => updateField('api_key', e.target.value)}
                                placeholder={t('providers.form.api_key_placeholder')}
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

                    {/* Available Models */}
                    <div>
                        <label className="block text-xs font-medium text-gray-600 dark:text-gray-400 mb-1">
                            {t('providers.form.available_models_label')}
                        </label>
                        <input
                            type="text"
                            className="input input-sm input-bordered w-full bg-white dark:bg-base-200 text-sm font-mono"
                            value={formData.available_models || ''}
                            onChange={e => updateField('available_models', e.target.value)}
                            placeholder={t('providers.form.available_models_placeholder')}
                        />
                        <p className="text-[10px] text-gray-400 dark:text-gray-500 mt-0.5">
                            {t('providers.form.available_models_hint', '逗号分隔，如 gemini-2.5-pro,gemini-2.5-flash')}
                        </p>
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
    const [modalData, setModalData] = useState<UpstreamProvider>(createEmptyProvider());
    const [deleteTarget, setDeleteTarget] = useState<UpstreamProvider | null>(null);
    const [testResults, setTestResults] = useState<{
        providerName: string;
        results: ModelTestResult[];
    } | null>(null);
    const [testingProvider, setTestingProvider] = useState<string | null>(null);

    const providers: UpstreamProvider[] = useMemo(() => {
        return config?.proxy?.providers ?? [];
    }, [config?.proxy?.providers]);

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
            result = result.filter((p) => p.name.toLowerCase().includes(lower));
        }
        if (protocolFilter) {
            result = result.filter((p) => p.protocol === protocolFilter);
        }
        return result;
    }, [providers, searchText, protocolFilter]);

    const handleAdd = () => {
        setEditingProvider(null);
        setModalData(createEmptyProvider());
        setIsModalOpen(true);
    };

    const handleEdit = (provider: UpstreamProvider) => {
        setEditingProvider(provider);
        setModalData({ ...provider });
        setIsModalOpen(true);
    };

    const handleDeleteConfirm = async () => {
        if (!deleteTarget || !config) return;
        const newProviders = providers.filter((p) => p.name !== deleteTarget.name);
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
        const newProviders = providers.map((p) =>
            p.name === provider.name ? { ...p, enabled: !p.enabled } : p
        );
        const newConfig = {
            ...config,
            proxy: {
                ...config.proxy,
                providers: newProviders,
            },
        };
        await saveConfig(newConfig);
    };

    const handleTestProvider = useCallback(async (provider: UpstreamProvider) => {
        if (!provider.available_models?.trim()) {
            showToast(t('providers.test.no_models'), 'warning');
            return;
        }
        setTestingProvider(provider.name);
        setTestResults(null);
        try {
            const data = await request<{ results: ModelTestResult[] }>('test_provider_models', {
                request: { provider },
            });
            setTestResults({ providerName: provider.name, results: data.results });
        } catch (e: any) {
            const msg = String(e?.message || e?.error || e);
            showToast(msg.includes('No models') ? t('providers.test.no_models') : msg, 'error');
        } finally {
            setTestingProvider(null);
        }
    }, [t]);

    const handleModalConfirm = async (values: UpstreamProvider, mappings: { from: string; to: string }[]) => {
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
            available_models: values.available_models || '',
            model_mapping,
        };

        // Check for duplicate name (only for new providers)
        if (!editingProvider && providers.some((p) => p.name === updated.name)) {
            showToast('Provider name already exists', 'error');
            return;
        }

        const newProviders: UpstreamProvider[] = editingProvider
            ? providers.map((p) =>
                p.name === editingProvider.name ? updated : p
            )
            : [...providers, updated];

        // Check provider_id uniqueness
        if (updated.provider_id) {
            const duplicate = editingProvider
                ? providers.some((p) => p.name !== editingProvider.name && p.provider_id === updated.provider_id)
                : providers.some((p) => p.provider_id === updated.provider_id);
            if (duplicate) {
                showToast('Provider ID already exists', 'error');
                return;
            }
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
                                    filteredProviders.map((provider) => (
                                        <tr
                                            key={provider.name}
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
                                                <span className={`inline-flex items-center px-2 py-0.5 rounded-md text-[10px] font-bold shadow-sm border ${PROTOCOL_COLORS[provider.protocol]}`}>
                                                    {t(`providers.protocols.${provider.protocol}`)}
                                                </span>
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
                                                        onClick={() => handleTestProvider(provider)}
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
                                    ))
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

            {/* Test Results Modal */}
            {testResults && createPortal(
                <div className="modal modal-open z-[100]">
                    <div className="modal-box relative max-w-2xl bg-white dark:bg-base-100 shadow-2xl rounded-2xl p-0 overflow-hidden max-h-[85vh] flex flex-col">
                        <div className="flex items-center justify-between px-6 pt-6 pb-4 border-b border-gray-100 dark:border-base-200">
                            <div>
                                <h3 className="text-lg font-bold text-gray-900 dark:text-base-content">
                                    {t('providers.test.results_title')} — {testResults.providerName}
                                </h3>
                                <p className="text-xs text-gray-500 dark:text-gray-400 mt-0.5">
                                    {t('providers.test.results_summary', {
                                        success: testResults.results.filter(r => r.success).length,
                                        total: testResults.results.length,
                                    })}
                                </p>
                            </div>
                            <button
                                className="btn btn-ghost btn-sm btn-circle text-gray-400 hover:text-gray-600 dark:hover:text-gray-300"
                                onClick={() => setTestResults(null)}
                            >
                                <X size={18} />
                            </button>
                        </div>

                        <div className="flex-1 overflow-y-auto px-6 py-4 space-y-2">
                            {testResults.results.map((r, i) => (
                                <div
                                    key={i}
                                    className="flex items-center gap-3 px-3 py-2 rounded-lg border text-sm"
                                    style={{
                                        borderColor: r.success ? 'rgb(34 197 94 / 0.3)' : 'rgb(239 68 68 / 0.3)',
                                        backgroundColor: r.success ? 'rgb(34 197 94 / 0.05)' : 'rgb(239 68 68 / 0.05)',
                                    }}
                                >
                                    {r.success ? (
                                        <CheckCircle2 size={16} className="text-green-500 flex-shrink-0" />
                                    ) : (
                                        <XCircle size={16} className="text-red-500 flex-shrink-0" />
                                    )}
                                    <code className="font-mono text-xs flex-1 truncate">{r.model}</code>
                                    <span className="text-xs text-gray-500 dark:text-gray-400 tabular-nums whitespace-nowrap">
                                        {(r.latency_ms || r.latencyMs || 0)}ms
                                    </span>
                                    {r.error && (
                                        <span className="text-xs text-red-500 dark:text-red-400 truncate max-w-[280px]" title={r.error}>
                                            {r.error}
                                        </span>
                                    )}
                                </div>
                            ))}
                        </div>

                        <div className="flex gap-3 px-6 py-4 border-t border-gray-100 dark:border-base-200">
                            <button
                                className="flex-1 px-4 py-2.5 bg-gray-100 dark:bg-base-200 text-gray-700 dark:text-gray-300 font-medium rounded-xl hover:bg-gray-200 dark:hover:bg-base-300 transition-colors"
                                onClick={() => setTestResults(null)}
                            >
                                {t('providers.test.close')}
                            </button>
                            <button
                                className="flex-1 px-4 py-2.5 bg-purple-500 hover:bg-purple-600 text-white font-medium rounded-xl shadow-md transition-all flex items-center justify-center gap-2"
                                onClick={() => {
                                    const provider = providers.find(p => p.name === testResults.providerName);
                                    if (provider) handleTestProvider(provider);
                                }}
                            >
                                <RotateCcw size={14} />
                                {t('providers.test.retest')}
                            </button>
                        </div>
                    </div>
                    <div className="modal-backdrop bg-black/40 backdrop-blur-sm fixed inset-0 z-[-1]" onClick={() => setTestResults(null)}></div>
                </div>,
                document.body
            )}
        </div>
    );
}

export default ProviderManager;
