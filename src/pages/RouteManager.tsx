import { useState, useEffect, useMemo } from 'react';
import { useTranslation } from 'react-i18next';
import { request as invoke } from '../utils/request';
import {
    BrainCircuit,
    Sparkles,
    ArrowRight,
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
} from 'lucide-react';
import { ProxyConfig, ExperimentalConfig, StickySessionConfig, WeightedTarget } from '../types/config';
import HelpTooltip from '../components/common/HelpTooltip';
import ModalDialog from '../components/common/ModalDialog';
import { showToast } from '../components/common/ToastContainer';
import GroupedSelect, { SelectOption } from '../components/common/GroupedSelect';
import DebouncedSlider from '../components/common/DebouncedSlider';
import AdvancedThinking from '../components/settings/AdvancedThinking';
import CircuitBreaker from '../components/settings/CircuitBreaker';
import { useProxyConfig } from '../hooks/useProxyConfig';
import { listAccounts } from '../services/accountService';

interface CustomPreset {
    id: string;
    name: string;
    description: string;
    mappings: Record<string, string | WeightedTarget[]>;
}

export default function RouteManager() {
    const { t } = useTranslation();
    const { appConfig, configLoading, configError, status, setAppConfig, saveConfig } = useProxyConfig();

    // Model routing state
    const [editingKey, setEditingKey] = useState<string | null>(null);
    const [editingValue, setEditingValue] = useState<string>('');
    const [selectedPreset, setSelectedPreset] = useState<string>('default');
    const [customPresets, setCustomPresets] = useState<CustomPreset[]>([]);
    const [isPresetManagerOpen, setIsPresetManagerOpen] = useState(false);
    const [newPresetName, setNewPresetName] = useState('');
    const [isResetConfirmOpen, setIsResetConfirmOpen] = useState(false);
    const [preferredAccountId, setPreferredAccountId] = useState<string | null>(null);
    const [availableAccounts, setAvailableAccounts] = useState<Array<{ id: string; email: string }>>([]);
    const [isClearBindingsConfirmOpen, setIsClearBindingsConfirmOpen] = useState(false);

    // Weighted mapping state
    const [weightedKey, setWeightedKey] = useState('');
    const [weightedTargets, setWeightedTargets] = useState<WeightedTarget[]>([{ target: '', weight: 50 }]);
    const [addMode, setAddMode] = useState<'single' | 'weighted'>('single');

    // Fallback model state
    const [fallbackEnabled, setFallbackEnabled] = useState(appConfig?.proxy?.fallback_model?.enabled ?? false);
    const [fallbackModel, setFallbackModel] = useState(appConfig?.proxy?.fallback_model?.model ?? '');
    const [fallbackProviderId, setFallbackProviderId] = useState(appConfig?.proxy?.fallback_model?.provider_id ?? '');

    // Cooldown state
    const [cooldowns, setCooldowns] = useState<Array<{ model: string; provider: string; remaining_secs: number }>>([]);
    const [cooldownEnabled, setCooldownEnabled] = useState(appConfig?.proxy?.model_cooldown?.enabled ?? true);
    const [cooldownDuration, setCooldownDuration] = useState(appConfig?.proxy?.model_cooldown?.duration_secs ?? 600);

    /** Resolve a mapping value to display string (handles both string and weighted array) */
    const resolveMappingDisplay = (val: string | WeightedTarget[]): string => {
        if (typeof val === 'string') return val;
        return val.map(t => `${t.target}(${t.weight})`).join(', ');
    };

    /** Get the string target for editing (first weighted target or the string itself) */
    const resolveMappingEditValue = (val: string | WeightedTarget[]): string => {
        if (typeof val === 'string') return val;
        return val.length > 0 ? val[0].target : '';
    };

    // Custom mapping options: only from provider available_models
    const customMappingOptions: SelectOption[] = useMemo(() => {
        const options: SelectOption[] = [];

        // Build options from each provider's available_models
        const providers = appConfig?.proxy?.providers?.filter(p => p.enabled && p.provider_id && p.available_models) ?? [];

        for (const provider of providers) {
            const pid = provider.provider_id!;
            const modelList = provider.available_models!.split(',').map(s => s.trim()).filter(Boolean);
            for (const modelName of modelList) {
                const value = `${pid}/${modelName}`;
                options.push({
                    value,
                    label: `${modelName} (${pid})`,
                    group: `供应商: ${provider.name}`,
                });
            }
        }

        return options;
    }, [appConfig?.proxy?.providers]);

    // Load custom presets from localStorage
    useEffect(() => {
        try {
            const saved = localStorage.getItem('antigravity_custom_presets');
            if (saved) {
                setCustomPresets(JSON.parse(saved));
            }
        } catch (error) {
            console.error('Failed to load custom presets:', error);
        }
        loadAccounts();
        loadPreferredAccount();
    }, []);

    // Load cooldowns on mount
    useEffect(() => {
        loadCooldowns();
    }, []);

    // Sync fallback/cooldown state from config
    useEffect(() => {
        if (appConfig?.proxy?.fallback_model) {
            setFallbackEnabled(appConfig.proxy.fallback_model.enabled);
            setFallbackModel(appConfig.proxy.fallback_model.model);
            setFallbackProviderId(appConfig.proxy.fallback_model.provider_id ?? '');
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
            localStorage.setItem('antigravity_custom_presets', JSON.stringify(presets));
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
            showToast(t('proxy.router.presets_applied') + ` (${selectedPresetData.name})`, 'success');
            await saveConfig({ ...appConfig, proxy: newConfig });
        } catch (error) {
            console.error('Failed to apply presets:', error);
            setAppConfig(oldConfig);
            showToast(`${t('common.error')}: ${error}`, 'error');
        }
    };

    const handleMappingUpdate = async (_type: 'custom', key: string, value: string) => {
        if (!appConfig) return false;
        const newConfig = { ...appConfig.proxy };
        newConfig.custom_mapping = { ...(newConfig.custom_mapping || {}), [key]: value };

        try {
            await saveConfig({ ...appConfig, proxy: newConfig });
            setAppConfig({ ...appConfig, proxy: newConfig });
            showToast(t('common.saved'), 'success');
            return true;
        } catch (error) {
            console.error('Failed to update mapping:', error);
            showToast(`${t('common.error')}: ${error}`, 'error');
            return false;
        }
    };

    const handleRemoveCustomMapping = async (key: string) => {
        if (!appConfig || !appConfig.proxy.custom_mapping) return;
        const newCustom = { ...appConfig.proxy.custom_mapping };
        delete newCustom[key];
        const newConfig = { ...appConfig.proxy, custom_mapping: newCustom };
        try {
            await saveConfig({ ...appConfig, proxy: newConfig });
            setAppConfig({ ...appConfig, proxy: newConfig });
        } catch (error) {
            console.error('Failed to remove custom mapping:', error);
        }
    };

    // --- Weighted mapping handlers ---
    const handleAddWeightedMapping = async () => {
        if (!appConfig || !weightedKey || weightedTargets.some(t => !t.target)) return;
        const newProxyConfig = {
            ...appConfig.proxy,
            custom_mapping: { ...(appConfig.proxy.custom_mapping || {}), [weightedKey]: weightedTargets }
        };
        try {
            await saveConfig({ ...appConfig, proxy: newProxyConfig });
            setWeightedKey('');
            setWeightedTargets([{ target: '', weight: 50 }]);
            showToast(t('common.saved'), 'success');
        } catch (error) {
            showToast(`${t('common.error')}: ${error}`, 'error');
        }
    };

    const handleAddWeightedTarget = () => {
        setWeightedTargets([...weightedTargets, { target: '', weight: 50 }]);
    };

    const handleRemoveWeightedTarget = (index: number) => {
        if (weightedTargets.length <= 1) return;
        setWeightedTargets(weightedTargets.filter((_, i) => i !== index));
    };

    const updateWeightedTarget = (index: number, field: keyof WeightedTarget, value: string | number) => {
        const updated = [...weightedTargets];
        updated[index] = { ...updated[index], [field]: value };
        setWeightedTargets(updated);
    };

    // --- Fallback model handlers ---
    const handleSaveFallbackModel = async () => {
        if (!appConfig) return;
        const newProxyConfig = {
            ...appConfig.proxy,
            fallback_model: { enabled: fallbackEnabled, model: fallbackModel, provider_id: fallbackProviderId }
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
            const data = await invoke<Array<{ model: string; provider: string; remaining_secs: number }>>('get_model_cooldowns');
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
            custom_mapping: {}
        };

        try {
            await saveConfig({ ...appConfig, proxy: newConfig });
            setAppConfig({ ...appConfig, proxy: newConfig });
            showToast(t('common.success'), 'success');
        } catch (error) {
            console.error('Failed to reset mapping:', error);
            showToast(`${t('common.error')}: ${error}`, 'error');
        }
    };

    const updateProxyConfig = (updates: Partial<ProxyConfig>) => {
        if (!appConfig) return;
        const newConfig = {
            ...appConfig,
            proxy: {
                ...appConfig.proxy,
                ...updates
            }
        };
        saveConfig(newConfig);
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
                        {/* Background Task Model Config */}
                        <div className="mb-4 pb-4 border-b border-gray-100 dark:border-base-200">
                            <div className="flex flex-col sm:flex-row sm:items-center justify-between gap-3">
                                <div className="flex-1">
                                    <h3 className="text-xs font-bold text-gray-700 dark:text-gray-300 flex items-center gap-2">
                                        <Sparkles size={14} className="text-blue-500" />
                                        {t('proxy.router.background_task_title')}
                                    </h3>
                                    <p className="text-[10px] text-gray-500 dark:text-gray-400 mt-0.5">
                                        {t('proxy.router.background_task_desc')}
                                    </p>
                                </div>

                                <div className="flex items-center gap-2 w-full sm:w-auto min-w-[200px] max-w-sm">
                                    <div className="relative flex-1">
                                        <GroupedSelect
                                            value={resolveMappingEditValue(appConfig.proxy.custom_mapping?.['internal-background-task'] || '')}
                                            onChange={(val) => handleMappingUpdate('custom', 'internal-background-task', val)}
                                            options={[
                                                { value: '', label: 'Default (auto-match provider)', group: 'System' },
                                                ...customMappingOptions
                                            ]}
                                            placeholder="Default (auto-match provider)"
                                            className="font-mono text-[11px] h-8 dark:bg-base-200 w-full"
                                        />
                                    </div>

                                    {appConfig.proxy.custom_mapping && appConfig.proxy.custom_mapping['internal-background-task'] && (
                                        <button
                                            onClick={() => handleRemoveCustomMapping('internal-background-task')}
                                            className="p-1.5 text-gray-400 hover:text-blue-500 hover:bg-blue-50 dark:hover:bg-blue-900/30 rounded transition-colors"
                                            title={t('proxy.router.use_default', { defaultValue: 'Use default' })}
                                        >
                                            <RefreshCw size={12} />
                                        </button>
                                    )}
                                </div>
                            </div>
                        </div>

                        {/* Custom Mapping List */}
                        <div className="flex flex-col gap-4">
                            <div className="w-full flex flex-col">
                                <div className="flex items-center justify-between mb-2">
                                    <span className="text-[10px] font-bold text-gray-400 dark:text-gray-500 uppercase tracking-wider">
                                        {t('proxy.router.current_list')}
                                    </span>
                                </div>
                                <div className="overflow-y-auto max-h-[500px] border border-gray-100 dark:border-white/5 rounded-lg bg-gray-50/10 dark:bg-white/5 p-3" data-custom-mapping-list>
                                    <div className="grid grid-cols-1 md:grid-cols-2 gap-x-6 gap-y-2">
                                        {appConfig.proxy.custom_mapping && Object.entries(appConfig.proxy.custom_mapping).length > 0 ? (
                                            Object.entries(appConfig.proxy.custom_mapping).map(([key, val]) => {
                                                const displayVal = resolveMappingDisplay(val);
                                                const editVal = resolveMappingEditValue(val);
                                                return (
                                                <div key={key} className={`flex items-center justify-between p-1.5 rounded-md transition-all border group ${editingKey === key ? 'bg-blue-50/80 dark:bg-blue-900/15 border-blue-300/50 dark:border-blue-500/30 shadow-sm' : 'border-transparent hover:bg-gray-100 dark:hover:bg-white/5 hover:border-gray-200 dark:hover:border-white/10'}`}>
                                                    <div className="flex items-center gap-2.5 overflow-hidden flex-1">
                                                        <span className="font-mono text-[10px] font-bold text-blue-600 dark:text-blue-400 truncate max-w-[140px]" title={key}>{key}</span>
                                                        <ArrowRight size={10} className="text-gray-300 dark:text-gray-600 shrink-0" />

                                                        {editingKey === key ? (
                                                            <div className="flex-1 mr-2">
                                                                <GroupedSelect
                                                                    value={editingValue}
                                                                    onChange={setEditingValue}
                                                                    options={customMappingOptions}
                                                                    placeholder="Select..."
                                                                    className="font-mono text-[10px] h-7 dark:bg-gray-800 border-blue-200 dark:border-blue-800"
                                                                    allowCustomInput={true}
                                                                />
                                                            </div>
                                                        ) : (
                                                            <span className="font-mono text-[10px] text-gray-500 dark:text-gray-400 truncate cursor-pointer hover:text-blue-500"
                                                                onClick={() => { setEditingKey(key); setEditingValue(editVal); }}
                                                                title={displayVal}>{displayVal}</span>
                                                        )}
                                                    </div>

                                                    <div className="flex items-center gap-1.5 shrink-0">
                                                        {editingKey === key ? (
                                                            <div className="flex items-center gap-1 bg-white dark:bg-gray-800 rounded-md border border-blue-200 dark:border-blue-800 p-0.5 shadow-sm">
                                                                <button
                                                                    className="btn btn-ghost btn-xs text-primary hover:bg-blue-50 dark:hover:bg-blue-900/30 p-0 h-6 w-6 min-h-0"
                                                                    onClick={async () => {
                                                                        const saved = await handleMappingUpdate('custom', key, editingValue);
                                                                        if (saved) setEditingKey(null);
                                                                    }}
                                                                    title={t('common.save') || 'Save'}
                                                                >
                                                                    <Check size={14} strokeWidth={3} />
                                                                </button>
                                                                <div className="w-[1px] h-3 bg-gray-200 dark:bg-gray-700" />
                                                                <button
                                                                    className="btn btn-ghost btn-xs text-gray-400 hover:bg-gray-100 dark:hover:bg-gray-700 p-0 h-6 w-6 min-h-0"
                                                                    onClick={() => setEditingKey(null)}
                                                                    title={t('common.cancel') || 'Cancel'}
                                                                >
                                                                    <X size={14} strokeWidth={3} />
                                                                </button>
                                                            </div>
                                                        ) : (
                                                            <div className="flex items-center gap-1 opacity-0 group-hover:opacity-100 transition-opacity">
                                                                <button
                                                                    className="btn btn-ghost btn-xs text-gray-400 hover:text-blue-500 hover:bg-blue-50 dark:hover:bg-white/10 p-0 h-6 w-6 min-h-0"
                                                                    onClick={() => { setEditingKey(key); setEditingValue(editVal); }}
                                                                    title={t('common.edit') || 'Edit'}
                                                                >
                                                                    <Edit2 size={12} />
                                                                </button>
                                                                <button
                                                                    className="btn btn-ghost btn-xs text-error hover:bg-red-50 dark:hover:bg-red-900/20 p-0 h-6 w-6 min-h-0"
                                                                    onClick={() => handleRemoveCustomMapping(key)}
                                                                    title={t('common.delete') || 'Delete'}
                                                                >
                                                                    <Trash2 size={12} />
                                                                </button>
                                                            </div>
                                                        )}
                                                    </div>
                                                </div>
                                            ); })
                                        ) : (
                                            <div className="col-span-full text-center py-4 text-gray-400 dark:text-gray-600 italic text-[11px]">{t('proxy.router.no_custom_mapping')}</div>
                                        )}
                                    </div>
                                </div>
                            </div>

                            {/* Weighted Mapping Add Form */}
                            <div className="w-full bg-gray-50/50 dark:bg-white/5 p-2.5 rounded-xl border border-gray-100 dark:border-white/5 shadow-inner">
                                <div className="flex items-center gap-2 mb-2">
                                    <Zap size={12} className="text-amber-500" />
                                    <span className="text-[10px] font-bold text-gray-400 dark:text-gray-500 uppercase tracking-wider">
                                        1对N 权重映射
                                    </span>
                                    <div className="flex gap-1 ml-auto">
                                        <button onClick={() => setAddMode('single')} className={`text-[10px] px-2 py-0.5 rounded ${addMode === 'single' ? 'bg-blue-500 text-white' : 'bg-gray-200 dark:bg-gray-700 text-gray-500'}`}>单目标</button>
                                        <button onClick={() => setAddMode('weighted')} className={`text-[10px] px-2 py-0.5 rounded ${addMode === 'weighted' ? 'bg-blue-500 text-white' : 'bg-gray-200 dark:bg-gray-700 text-gray-500'}`}>多目标</button>
                                    </div>
                                </div>

                                {addMode === 'single' ? (
                                    <div className="flex flex-col sm:flex-row items-center gap-2">
                                        <input type="text" placeholder="原始模型名 (如 gpt-4*)" className="input input-xs input-bordered flex-1 font-mono text-[11px] bg-white dark:bg-gray-800 h-8"
                                            onChange={e => { setWeightedKey(e.target.value); }} value={weightedKey} />
                                        <div className="w-full sm:w-48">
                                            <GroupedSelect value={weightedTargets[0]?.target || ''} onChange={v => updateWeightedTarget(0, 'target', v)} options={customMappingOptions} placeholder="目标模型" className="font-mono text-[11px] h-8 dark:bg-gray-800" allowCustomInput={true} />
                                        </div>
                                        <button className="btn btn-xs sm:w-20 gap-1.5 shadow-md hover:shadow-lg transition-all bg-blue-600 hover:bg-blue-700 text-white border-none h-8" onClick={handleAddWeightedMapping}><Plus size={14} />{t('common.add')}</button>
                                    </div>
                                ) : (
                                    <div className="space-y-2">
                                        <input type="text" placeholder="原始模型名 (如 claude-sonnet-*)" className="input input-xs input-bordered w-full font-mono text-[11px] bg-white dark:bg-gray-800 h-8"
                                            onChange={e => setWeightedKey(e.target.value)} value={weightedKey} />
                                        {weightedTargets.map((wt, idx) => (
                                            <div key={idx} className="flex items-center gap-2">
                                                <span className="text-[10px] text-gray-400 w-6">#{idx + 1}</span>
                                                <div className="flex-1">
                                                    <GroupedSelect value={wt.target} onChange={v => updateWeightedTarget(idx, 'target', v)} options={customMappingOptions} placeholder="目标模型" className="font-mono text-[11px] h-8 dark:bg-gray-800 w-full" allowCustomInput={true} />
                                                </div>
                                                <input type="number" min={1} max={100} value={wt.weight} onChange={e => updateWeightedTarget(idx, 'weight', parseInt(e.target.value) || 1)} className="input input-xs input-bordered w-16 font-mono text-[11px] bg-white dark:bg-gray-800 h-8 text-center" title="权重" />
                                                <button onClick={() => handleRemoveWeightedTarget(idx)} className="btn btn-ghost btn-xs text-error p-0 h-6 w-6 min-h-0" disabled={weightedTargets.length <= 1}><X size={12} /></button>
                                            </div>
                                        ))}
                                        <div className="flex gap-2 justify-end">
                                            <button onClick={handleAddWeightedTarget} className="btn btn-xs btn-ghost gap-1"><Plus size={12} />添加目标</button>
                                            <button className="btn btn-xs sm:w-20 gap-1.5 shadow-md hover:shadow-lg transition-all bg-blue-600 hover:bg-blue-700 text-white border-none h-8" onClick={handleAddWeightedMapping}><Plus size={14} />{t('common.add')}</button>
                                        </div>
                                    </div>
                                )}
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
                            <p className="text-[11px] text-gray-500 dark:text-gray-400 mt-1">当请求的模型处于冷却状态时，自动转换到指定的兜底模型</p>
                        </div>
                        <label className="relative inline-flex items-center cursor-pointer">
                            <input type="checkbox" className="sr-only peer" checked={fallbackEnabled} onChange={e => setFallbackEnabled(e.target.checked)} />
                            <div className="w-11 h-6 bg-gray-200 dark:bg-base-300 peer-focus:outline-none rounded-full peer peer-checked:after:translate-x-full peer-checked:after:border-white after:content-[''] after:absolute after:top-[2px] after:left-[2px] after:bg-white after:border-gray-300 after:border after:rounded-full after:h-5 after:w-5 after:transition-all peer-checked:bg-amber-500 shadow-inner"></div>
                        </label>
                    </div>
                    {fallbackEnabled && (
                        <div className="flex flex-col sm:flex-row gap-3">
                            <div className="flex-1">
                                <label className="text-[10px] text-gray-400 mb-1 block">兜底模型</label>
                                <GroupedSelect value={fallbackModel} onChange={setFallbackModel} options={customMappingOptions} placeholder="输入或选择模型" className="font-mono text-[11px] h-9 dark:bg-base-200 w-full" allowCustomInput={true} />
                            </div>
                            <div className="flex items-end">
                                <button className="btn btn-sm sm:w-20 bg-amber-600 hover:bg-amber-700 text-white gap-1 h-9" onClick={handleSaveFallbackModel}><Save size={14} />保存</button>
                            </div>
                        </div>
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
                            <p className="text-[11px] text-gray-500 dark:text-gray-400 mt-1">遇到 429 错误的模型会进入冷却，冷却期间请求自动走兜底模型</p>
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
                                        <div key={`${cd.model}-${cd.provider}-${idx}`} className="flex items-center justify-between px-3 py-2 bg-red-50/50 dark:bg-red-900/10 rounded-md border border-red-100 dark:border-red-900/30">
                                            <div className="flex items-center gap-3">
                                                <Activity size={12} className="text-red-500 animate-pulse" />
                                                <span className="font-mono text-[11px] text-gray-700 dark:text-gray-300">{cd.model}</span>
                                                <span className="text-[10px] text-gray-400">@{cd.provider}</span>
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

                {/* Advanced Thinking & Global Config */}
                <AdvancedThinking
                    config={appConfig.proxy}
                    onChange={(newProxyConfig) => updateProxyConfig(newProxyConfig)}
                />

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
