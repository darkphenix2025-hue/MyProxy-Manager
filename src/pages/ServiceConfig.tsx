import { useState, useMemo, useEffect } from 'react';
import { useTranslation } from 'react-i18next';
import { request as invoke } from '../utils/request';
import { isTauri } from '../utils/env';
import { copyToClipboard } from '../utils/clipboard';
import {
    Power,
    Copy,
    RefreshCw,
    CheckCircle,
    Settings,
    Terminal,
    Puzzle,
    Code,
    X,
    Edit2,
} from 'lucide-react';
import { ProxyConfig } from '../types/config';
import HelpTooltip from '../components/common/HelpTooltip';
import { showToast } from '../components/common/ToastContainer';
import { cn } from '../utils/cn';
import { useProxyModels } from '../hooks/useProxyModels';
import { useProxyConfig } from '../hooks/useProxyConfig';
import GroupedSelect, { SelectOption } from '../components/common/GroupedSelect';
import { CliSyncCard } from '../components/proxy/CliSyncCard';

export default function ServiceConfig() {
    const { t } = useTranslation();
    const { models } = useProxyModels();
    const {
        appConfig, configLoading, configError, status, loading,
        saveConfig, handleToggle,
    } = useProxyConfig();

    const [copied, setCopied] = useState<string | null>(null);
    const [selectedProtocol, setSelectedProtocol] = useState<'openai' | 'anthropic' | 'gemini'>('openai');
    const [selectedModelId, setSelectedModelId] = useState('gemini-3-flash');

    // API Key editing states
    const [isEditingApiKey, setIsEditingApiKey] = useState(false);
    const [tempApiKey, setTempApiKey] = useState('');

    const [isEditingAdminPassword, setIsEditingAdminPassword] = useState(false);
    const [tempAdminPassword, setTempAdminPassword] = useState('');

    // Cloudflared (CF隧道) states
    const [cfStatus, setCfStatus] = useState<{ installed: boolean; version?: string; running: boolean; url?: string; error?: string }>({
        installed: false,
        running: false,
    });
    const [cfLoading, setCfLoading] = useState(false);
    const [cfMode, setCfMode] = useState<'quick' | 'auth'>('quick');
    const [cfToken, setCfToken] = useState('');
    const [cfUseHttp2, setCfUseHttp2] = useState(true);

    const updateZaiMcpConfig = (mcpUpdates: Partial<NonNullable<ProxyConfig['zai']>['mcp']>) => {
        if (!appConfig?.proxy.zai) return;
        const newConfig = {
            ...appConfig,
            proxy: {
                ...appConfig.proxy,
                zai: {
                    ...appConfig.proxy.zai,
                    mcp: { ...(appConfig.proxy.zai.mcp || {}), ...mcpUpdates }
                }
            }
        };
        saveConfig(newConfig);
    };

    const copyToClipboardHandler = async (text: string, key: string) => {
        const success = await copyToClipboard(text);
        if (success) {
            setCopied(key);
            setTimeout(() => setCopied(null), 2000);
        }
    };

    const updateProxyConfig = async (partial: Partial<ProxyConfig>) => {
        if (!appConfig) return;
        const newConfig = { ...appConfig, proxy: { ...appConfig.proxy, ...partial } };
        await saveConfig(newConfig);
    };

    // API Key handlers
    const handleEditApiKey = () => {
        setTempApiKey(appConfig?.proxy.api_key || '');
        setIsEditingApiKey(true);
    };
    const handleSaveApiKey = async () => {
        if (!tempApiKey.trim()) return;
        await updateProxyConfig({ api_key: tempApiKey });
        setIsEditingApiKey(false);
        showToast(t('common.saved'), 'success');
    };
    const handleCancelEditApiKey = () => { setTempApiKey(''); setIsEditingApiKey(false); };
    const handleGenerateApiKey = async () => {
        const key = `sk-${crypto.randomUUID()}`;
        await updateProxyConfig({ api_key: key });
        showToast(t('proxy.config.api_key_regenerated'), 'success');
    };

    // Admin Password handlers
    const handleEditAdminPassword = () => {
        setTempAdminPassword(appConfig?.proxy.admin_password || '');
        setIsEditingAdminPassword(true);
    };
    const handleSaveAdminPassword = () => {
        if (tempAdminPassword && tempAdminPassword.length < 4) {
            showToast(t('proxy.config.admin_password_short', { defaultValue: 'Password is too short (min 4 chars)' }), 'error');
            return;
        }
        updateProxyConfig({ admin_password: tempAdminPassword || undefined });
        // 同步更新 sessionStorage 中的密钥，避免 401 错误
        if (tempAdminPassword) {
            sessionStorage.setItem('abv_admin_api_key', tempAdminPassword);
        }
        setIsEditingAdminPassword(false);
        showToast(t('proxy.config.admin_password_updated', { defaultValue: 'Web UI password updated' }), 'success');
    };
    const handleCancelEditAdminPassword = () => { setTempAdminPassword(''); setIsEditingAdminPassword(false); };

    // Cloudflared handlers
    const loadCfStatus = async () => {
        try {
            const status = await invoke<typeof cfStatus>('cloudflared_get_status');
            setCfStatus(status);
        } catch (error) {
            // Ignore errors (e.g., manager not initialized)
        }
    };

    const handleCfInstall = async () => {
        setCfLoading(true);
        try {
            const status = await invoke<typeof cfStatus>('cloudflared_install');
            setCfStatus(status);
            showToast(t('proxy.cloudflared.install_success', { defaultValue: 'Cloudflared installed successfully' }), 'success');
        } catch (error) {
            showToast(String(error), 'error');
        } finally {
            setCfLoading(false);
        }
    };

    const handleCfToggle = async (enable: boolean) => {
        if (enable && !status.running) {
            showToast(
                t('proxy.cloudflared.require_proxy_running', { defaultValue: 'Please start the local proxy service first' }),
                'warning'
            );
            return;
        }
        setCfLoading(true);
        try {
            if (enable) {
                if (!cfStatus.installed) {
                    const installStatus = await invoke<typeof cfStatus>('cloudflared_install');
                    setCfStatus(installStatus);
                    if (!installStatus.installed) {
                        throw new Error('Cloudflared install failed');
                    }
                    showToast(t('proxy.cloudflared.install_success', { defaultValue: 'Cloudflared installed successfully' }), 'success');
                }
                const config = {
                    enabled: true,
                    mode: cfMode,
                    port: appConfig?.proxy.port || 8150,
                    token: cfMode === 'auth' ? cfToken : null,
                    use_http2: cfUseHttp2,
                };
                const status = await invoke<typeof cfStatus>('cloudflared_start', { config });
                setCfStatus(status);
                showToast(t('proxy.cloudflared.started', { defaultValue: 'Tunnel started' }), 'success');
                if (appConfig) {
                    saveConfig({
                        ...appConfig,
                        cloudflared: {
                            ...appConfig.cloudflared,
                            enabled: true,
                            mode: cfMode,
                            token: cfToken,
                            use_http2: cfUseHttp2,
                            port: appConfig.proxy.port || 8150
                        }
                    });
                }
            } else {
                const status = await invoke<typeof cfStatus>('cloudflared_stop');
                setCfStatus(status);
                showToast(t('proxy.cloudflared.stopped', { defaultValue: 'Tunnel stopped' }), 'success');
                if (appConfig) {
                    saveConfig({
                        ...appConfig,
                        cloudflared: { ...appConfig.cloudflared, enabled: false }
                    });
                }
            }
        } catch (error) {
            showToast(String(error), 'error');
        } finally {
            setCfLoading(false);
        }
    };

    const handleCfCopyUrl = async () => {
        if (cfStatus.url) {
            const success = await copyToClipboard(cfStatus.url);
            if (success) {
                setCopied('cf-url');
                setTimeout(() => setCopied(null), 2000);
            }
        }
    };

    // Cloudflared status polling
    useEffect(() => {
        loadCfStatus();
        const interval = setInterval(loadCfStatus, 5000);
        return () => clearInterval(interval);
    }, []);

    // Restore Cloudflared persisted state from config
    useEffect(() => {
        if (appConfig?.cloudflared) {
            setCfMode(appConfig.cloudflared.mode || 'quick');
            setCfToken(appConfig.cloudflared.token || '');
            setCfUseHttp2(appConfig.cloudflared.use_http2 !== false);
        }
    }, [appConfig]);

    const customMappingOptions: SelectOption[] = useMemo(() => {
        return models.map(model => ({
            value: model.id,
            label: `${model.id} (${model.name})`,
            group: model.group || 'Other'
        }));
    }, [models]);

    const getPythonExample = (modelId: string) => {
        const port = status.running ? status.port : (appConfig?.proxy.port || 8150);
        const baseUrl = `http://127.0.0.1:${port}/v1`;
        const apiKey = appConfig?.proxy.api_key || 'YOUR_API_KEY';

        if (selectedProtocol === 'anthropic') {
            return `from anthropic import Anthropic

client = Anthropic(
    base_url="http://127.0.0.1:${port}",
    api_key="${apiKey}"
)

response = client.messages.create(
    model="${modelId}",
    max_tokens=1024,
    messages=[{"role": "user", "content": "Hello"}]
)

print(response.content[0].text)`;
        }

        if (selectedProtocol === 'gemini') {
            return `# pip install google-generativeai
import google.generativeai as genai

genai.configure(
    api_key="${apiKey}",
    transport='rest',
    client_options={'api_endpoint': 'http://127.0.0.1:${port}'}
)

model = genai.GenerativeModel('${modelId}')
response = model.generate_content("Hello")
print(response.text)`;
        }

        if (modelId.startsWith('gemini-3-pro-image')) {
            return `from openai import OpenAI

client = OpenAI(
    base_url="${baseUrl}",
    api_key="${apiKey}"
)

response = client.chat.completions.create(
    model="${modelId}",
    extra_body={ "size": "1024x1024" },
    messages=[{"role": "user", "content": "Draw a futuristic city"}]
)

print(response.choices[0].message.content)`;
        }

        return `from openai import OpenAI

client = OpenAI(
    base_url="${baseUrl}",
    api_key="${apiKey}"
)

response = client.chat.completions.create(
    model="${modelId}",
    messages=[{"role": "user", "content": "Hello"}]
)

print(response.choices[0].message.content)`;
    };

    const filteredModels = models.filter(model => {
        if (selectedProtocol === 'openai') return true;
        if (selectedProtocol === 'anthropic') return !model.id.includes('image');
        return true;
    });

    if (configLoading) {
        return (
            <div className="h-full w-full overflow-y-auto">
                <div className="p-5 flex items-center justify-center min-h-[60vh]">
                    <div className="text-center">
                        <RefreshCw size={32} className="animate-spin text-blue-500 mx-auto mb-3" />
                        <p className="text-sm text-gray-500">{t('common.loading')}</p>
                    </div>
                </div>
            </div>
        );
    }

    if (configError) {
        return (
            <div className="h-full w-full overflow-y-auto">
                <div className="p-5 max-w-7xl mx-auto">
                    <div className="bg-red-50 dark:bg-red-900/20 border border-red-200 dark:border-red-800 rounded-xl p-4 text-center">
                        <p className="text-sm text-red-600 dark:text-red-400 mb-3">{t('common.error')}: {configError}</p>
                        <button
                            onClick={() => window.location.reload()}
                            className="px-4 py-2 bg-red-500 text-white text-sm font-medium rounded-lg hover:bg-red-600 transition-colors"
                        >
                            {t('common.retry')}
                        </button>
                    </div>
                </div>
            </div>
        );
    }

    if (!appConfig) return null;

    return (
        <div className="h-full w-full overflow-y-auto overflow-x-hidden">
            <div className="p-5 space-y-4 max-w-7xl mx-auto">
                {/* 服务基础配置 */}
                <div className="bg-white dark:bg-base-100 rounded-xl shadow-sm border border-gray-100 dark:border-base-200">
                    <div className="px-4 py-2.5 border-b border-gray-100 dark:border-base-200 flex items-center justify-between">
                        <div className="flex items-center gap-4">
                            <h2 className="text-base font-semibold flex items-center gap-2 text-gray-900 dark:text-base-content">
                                <Settings size={18} />
                                {t('proxy.config.title')}
                            </h2>
                            <div className="flex items-center gap-2 pl-4 border-l border-gray-200 dark:border-base-300">
                                <div className={`w-2 h-2 rounded-full ${status.running ? 'bg-green-500 animate-pulse' : 'bg-gray-400'}`} />
                                <span className={`text-xs font-medium ${status.running ? 'text-green-600' : 'text-gray-500'}`}>
                                    {status.running
                                        ? `${t('proxy.status.running')} (${status.active_accounts} ${t('common.accounts')})`
                                        : t('proxy.status.stopped')}
                                </span>
                            </div>
                        </div>
                        <div className="flex items-center gap-2">
                            <button
                                onClick={handleToggle}
                                disabled={loading || !appConfig}
                                className={`px-3 py-1 rounded-lg text-xs font-medium transition-colors flex items-center gap-2 ${status.running
                                    ? 'bg-red-50 to-red-600 text-red-600 hover:bg-red-100 border border-red-200'
                                    : 'bg-blue-600 hover:bg-blue-700 text-white shadow-sm shadow-blue-500/30'
                                    } ${(loading || !appConfig) ? 'opacity-50 cursor-not-allowed' : ''}`}
                            >
                                <Power size={14} />
                                {loading ? t('proxy.status.processing') : (status.running ? t('proxy.action.stop') : t('proxy.action.start'))}
                            </button>
                        </div>
                    </div>
                    <div className="p-3 space-y-3">
                        {/* 监听端口、超时和自启动 */}
                        <div className="grid grid-cols-1 md:grid-cols-3 gap-3">
                            <div>
                                <label className="block text-xs font-medium text-gray-700 dark:text-gray-300 mb-1">
                                    <span className="inline-flex items-center gap-1">
                                        {t('proxy.config.port')}
                                        <HelpTooltip text={t('proxy.config.port_tooltip')} ariaLabel={t('proxy.config.port')} placement="right" />
                                    </span>
                                </label>
                                <input type="number" value={appConfig.proxy.port}
                                    onChange={(e) => updateProxyConfig({ port: parseInt(e.target.value) })}
                                    min={8000} max={65535} disabled={status.running}
                                    className="w-full px-2.5 py-1.5 border border-gray-300 dark:border-base-200 rounded-lg bg-white dark:bg-base-200 text-xs text-gray-900 dark:text-base-content focus:ring-2 focus:ring-blue-500 focus:border-transparent disabled:opacity-50 disabled:cursor-not-allowed" />
                                <p className="mt-0.5 text-[10px] text-gray-500 dark:text-gray-400">{t('proxy.config.port_hint')}</p>
                            </div>
                            <div>
                                <label className="block text-xs font-medium text-gray-700 dark:text-gray-300 mb-1">
                                    <span className="inline-flex items-center gap-1">
                                        {t('proxy.config.request_timeout')}
                                        <HelpTooltip text={t('proxy.config.request_timeout_tooltip')} ariaLabel={t('proxy.config.request_timeout')} placement="top" />
                                    </span>
                                </label>
                                <input type="number" value={appConfig.proxy.request_timeout || 120}
                                    onChange={(e) => { const v = parseInt(e.target.value); updateProxyConfig({ request_timeout: Math.max(30, Math.min(7200, v)) }); }}
                                    min={30} max={7200} disabled={status.running}
                                    className="w-full px-2.5 py-1.5 border border-gray-300 dark:border-base-200 rounded-lg bg-white dark:bg-base-200 text-xs text-gray-900 dark:text-base-content focus:ring-2 focus:ring-blue-500 focus:border-transparent disabled:opacity-50 disabled:cursor-not-allowed" />
                                <p className="mt-0.5 text-[10px] text-gray-500 dark:text-gray-400">{t('proxy.config.request_timeout_hint')}</p>
                            </div>
                            <div className="flex items-center">
                                <label className="flex items-center cursor-pointer gap-3">
                                    <input type="checkbox" className="toggle toggle-sm bg-gray-200 dark:bg-gray-700 border-gray-300 dark:border-gray-600 checked:bg-blue-500 checked:border-blue-500 disabled:opacity-50 disabled:bg-gray-100 dark:disabled:bg-gray-800"
                                        checked={appConfig.proxy.auto_start}
                                        onChange={(e) => updateProxyConfig({ auto_start: e.target.checked })} />
                                    <span className="text-xs font-medium text-gray-900 dark:text-base-content inline-flex items-center gap-1">
                                        {t('proxy.config.auto_start')}
                                        <HelpTooltip text={t('proxy.config.auto_start_tooltip')} ariaLabel={t('proxy.config.auto_start')} placement="right" />
                                    </span>
                                </label>
                            </div>
                        </div>

                        {/* 局域网访问 & 访问授权 */}
                        <div className="border-t border-gray-200 dark:border-base-300 pt-3 mt-3">
                            <div className="grid grid-cols-1 lg:grid-cols-2 gap-4">
                                <div className="space-y-2">
                                    <div className="flex items-center justify-between">
                                        <span className="text-xs font-medium text-gray-700 dark:text-gray-300 inline-flex items-center gap-1">
                                            {t('proxy.config.allow_lan_access')}
                                            <HelpTooltip text={t('proxy.config.allow_lan_access_tooltip')} ariaLabel={t('proxy.config.allow_lan_access')} placement="right" />
                                        </span>
                                        <input type="checkbox" className="toggle toggle-sm bg-gray-200 dark:bg-gray-700 border-gray-300 dark:border-gray-600 checked:bg-blue-500 checked:border-blue-500"
                                            checked={appConfig.proxy.allow_lan_access || false}
                                            onChange={(e) => updateProxyConfig({ allow_lan_access: e.target.checked })} />
                                    </div>
                                    <p className="text-[10px] text-gray-500 dark:text-gray-400">
                                        {(appConfig.proxy.allow_lan_access || false) ? t('proxy.config.allow_lan_access_hint_enabled') : t('proxy.config.allow_lan_access_hint_disabled')}
                                    </p>
                                    {(appConfig.proxy.allow_lan_access || false) && (
                                        <p className="text-[10px] text-amber-600 dark:text-amber-500">{t('proxy.config.allow_lan_access_warning')}</p>
                                    )}
                                    {status.running && (
                                        <p className="text-[10px] text-blue-600 dark:text-blue-400">{t('proxy.config.allow_lan_access_restart_hint')}</p>
                                    )}
                                </div>
                                <div className="space-y-2">
                                    <div className="flex items-center justify-between">
                                        <label className="text-xs font-medium text-gray-700 dark:text-gray-300">
                                            <span className="inline-flex items-center gap-1">
                                                {t('proxy.config.auth.title')}
                                                <HelpTooltip text={t('proxy.config.auth.title_tooltip')} ariaLabel={t('proxy.config.auth.title')} placement="top" />
                                            </span>
                                        </label>
                                        <label className="flex items-center cursor-pointer gap-2">
                                            <span className="text-[11px] text-gray-600 dark:text-gray-400 inline-flex items-center gap-1">
                                                {(appConfig.proxy.auth_mode || 'off') !== 'off' ? t('proxy.config.auth.enabled') : t('common.disabled')}
                                                <HelpTooltip text={t('proxy.config.auth.enabled_tooltip')} ariaLabel={t('proxy.config.auth.enabled')} placement="left" />
                                            </span>
                                            <input type="checkbox" className="toggle toggle-sm bg-gray-200 dark:bg-gray-700 border-gray-300 dark:border-gray-600 checked:bg-blue-500 checked:border-blue-500 disabled:opacity-50 disabled:bg-gray-100 dark:disabled:bg-gray-800"
                                                checked={(appConfig.proxy.auth_mode || 'off') !== 'off'}
                                                onChange={(e) => updateProxyConfig({ auth_mode: e.target.checked ? 'all_except_health' : 'off' })} />
                                        </label>
                                    </div>
                                    <div>
                                        <label className="block text-[11px] text-gray-600 dark:text-gray-400 mb-1">
                                            <span className="inline-flex items-center gap-1">
                                                {t('proxy.config.auth.mode')}
                                                <HelpTooltip text={t('proxy.config.auth.mode_tooltip')} ariaLabel={t('proxy.config.auth.mode')} placement="top" />
                                            </span>
                                        </label>
                                        <select value={appConfig.proxy.auth_mode || 'off'}
                                            onChange={(e) => updateProxyConfig({ auth_mode: e.target.value as ProxyConfig['auth_mode'] })}
                                            className="w-full px-2.5 py-1.5 border border-gray-300 dark:border-base-200 rounded-lg bg-white dark:bg-base-200 text-xs text-gray-900 dark:text-base-content focus:ring-2 focus:ring-blue-500 focus:border-transparent">
                                            <option value="off">{t('proxy.config.auth.modes.off')}</option>
                                            <option value="strict">{t('proxy.config.auth.modes.strict')}</option>
                                            <option value="all_except_health">{t('proxy.config.auth.modes.all_except_health')}</option>
                                            <option value="auto">{t('proxy.config.auth.modes.auto')}</option>
                                        </select>
                                        <p className="mt-0.5 text-[10px] text-gray-500 dark:text-gray-400">{t('proxy.config.auth.hint')}</p>
                                    </div>
                                </div>
                            </div>
                        </div>

                        {/* API 密钥 */}
                        <div>
                            <label className="block text-xs font-medium text-gray-700 dark:text-gray-300 mb-1">
                                <span className="inline-flex items-center gap-1">
                                    {t('proxy.config.api_key')}
                                    <HelpTooltip text={t('proxy.config.api_key_tooltip')} ariaLabel={t('proxy.config.api_key')} placement="right" />
                                </span>
                            </label>
                            <div className="flex gap-2">
                                <input type="text"
                                    value={isEditingApiKey ? tempApiKey : (appConfig.proxy.api_key)}
                                    onChange={(e) => isEditingApiKey && setTempApiKey(e.target.value)}
                                    readOnly={!isEditingApiKey}
                                    className={`flex-1 px-2.5 py-1.5 border border-gray-300 dark:border-base-200 rounded-lg text-xs font-mono ${isEditingApiKey ? 'bg-white dark:bg-base-200 text-gray-900 dark:text-base-content' : 'bg-gray-50 dark:bg-base-300 text-gray-600 dark:text-gray-400'}`} />
                                {isEditingApiKey ? (<>
                                    <button onClick={handleSaveApiKey} className="px-2.5 py-1.5 border border-green-300 dark:border-green-700 rounded-lg bg-green-50 dark:bg-green-900/20 hover:bg-green-100 dark:hover:bg-green-900/30 transition-colors text-green-600 dark:text-green-400" title={t('proxy.config.btn_save')}><CheckCircle size={14} /></button>
                                    <button onClick={handleCancelEditApiKey} className="px-2.5 py-1.5 border border-gray-300 dark:border-base-200 rounded-lg bg-white dark:bg-base-200 hover:bg-gray-50 dark:hover:bg-base-300 transition-colors" title={t('common.cancel')}><X size={14} /></button>
                                </>) : (<>
                                    <button onClick={handleEditApiKey} className="px-2.5 py-1.5 border border-gray-300 dark:border-base-200 rounded-lg bg-white dark:bg-base-200 hover:bg-gray-50 dark:hover:bg-base-300 transition-colors" title={t('proxy.config.btn_edit')}><Edit2 size={14} /></button>
                                    <button onClick={handleGenerateApiKey} className="px-2.5 py-1.5 border border-gray-300 dark:border-base-200 rounded-lg bg-white dark:bg-base-200 hover:bg-gray-50 dark:hover:bg-base-300 transition-colors" title={t('proxy.config.btn_regenerate')}><RefreshCw size={14} /></button>
                                    <button onClick={() => copyToClipboardHandler(appConfig.proxy.api_key, 'api_key')} className="px-2.5 py-1.5 border border-gray-300 dark:border-base-200 rounded-lg bg-white dark:bg-base-200 hover:bg-gray-50 dark:hover:bg-base-300 transition-colors" title={t('proxy.config.btn_copy')}>
                                        {copied === 'api_key' ? <CheckCircle size={14} className="text-green-500" /> : <Copy size={14} />}
                                    </button>
                                </>)}
                            </div>
                            <p className="mt-0.5 text-[10px] text-amber-600 dark:text-amber-500">{t('proxy.config.warning_key')}</p>
                        </div>

                        {/* Web UI 管理密码 */}
                        <div className="border-t border-gray-200 dark:border-base-300 pt-3 mt-3">
                            <label className="block text-xs font-medium text-gray-700 dark:text-gray-300 mb-1">
                                <span className="inline-flex items-center gap-1">
                                    {t('proxy.config.admin_password', { defaultValue: 'Web UI Login Password' })}
                                    <HelpTooltip text={t('proxy.config.admin_password_tooltip', { defaultValue: 'Used for logging into the Web Management Console. If empty, it defaults to the API Key.' })} ariaLabel={t('proxy.config.admin_password')} placement="right" />
                                </span>
                            </label>
                            <div className="flex gap-2">
                                <input type="text"
                                    value={isEditingAdminPassword ? tempAdminPassword : (appConfig.proxy.admin_password || t('proxy.config.admin_password_default', { defaultValue: '(Same as API Key)' }))}
                                    onChange={(e) => isEditingAdminPassword && setTempAdminPassword(e.target.value)}
                                    readOnly={!isEditingAdminPassword}
                                    placeholder={t('proxy.config.admin_password_placeholder', { defaultValue: 'Enter new password or leave empty to use API Key' })}
                                    className={`flex-1 px-2.5 py-1.5 border border-gray-300 dark:border-base-200 rounded-lg text-xs font-mono ${isEditingAdminPassword ? 'bg-white dark:bg-base-200 text-gray-900 dark:text-base-content' : 'bg-gray-50 dark:bg-base-300 text-gray-600 dark:text-gray-400'}`} />
                                {isEditingAdminPassword ? (<>
                                    <button onClick={handleSaveAdminPassword} className="px-2.5 py-1.5 border border-green-300 dark:border-green-700 rounded-lg bg-green-50 dark:bg-green-900/20 hover:bg-green-100 dark:hover:bg-green-900/30 transition-colors text-green-600 dark:text-green-400" title={t('proxy.config.btn_save')}><CheckCircle size={14} /></button>
                                    <button onClick={handleCancelEditAdminPassword} className="px-2.5 py-1.5 border border-gray-300 dark:border-base-200 rounded-lg bg-white dark:bg-base-200 hover:bg-gray-50 dark:hover:bg-base-300 transition-colors" title={t('common.cancel')}><X size={14} /></button>
                                </>) : (<>
                                    <button onClick={handleEditAdminPassword} className="px-2.5 py-1.5 border border-gray-300 dark:border-base-200 rounded-lg bg-white dark:bg-base-200 hover:bg-gray-50 dark:hover:bg-base-300 transition-colors" title={t('proxy.config.btn_edit')}><Edit2 size={14} /></button>
                                    <button onClick={() => copyToClipboardHandler(appConfig.proxy.admin_password || appConfig.proxy.api_key, 'admin_password')} className="px-2.5 py-1.5 border border-gray-300 dark:border-base-200 rounded-lg bg-white dark:bg-base-200 hover:bg-gray-50 dark:hover:bg-base-300 transition-colors" title={t('proxy.config.btn_copy')}>
                                        {copied === 'admin_password' ? <CheckCircle size={14} className="text-green-500" /> : <Copy size={14} />}
                                    </button>
                                </>)}
                            </div>
                            <p className="mt-0.5 text-[10px] text-gray-500 dark:text-gray-400">{t('proxy.config.admin_password_hint', { defaultValue: 'For safety in Docker/Browser environments, you can set a separate login password from your API Key.' })}</p>
                        </div>

                        {/* User-Agent Overrides */}
                        <div className="border-t border-gray-200 dark:border-base-300 pt-3 mt-3">
                            <div className="flex items-center justify-between mb-2">
                                <label className="text-xs font-medium text-gray-700 dark:text-gray-300 inline-flex items-center gap-1">
                                    {t('proxy.config.request.user_agent', { defaultValue: 'User-Agent Override' })}
                                    <HelpTooltip text={t('proxy.config.request.user_agent_tooltip', { defaultValue: 'Override the User-Agent header sent to upstream APIs.' })} />
                                </label>
                                <input type="checkbox" className="toggle toggle-sm bg-gray-200 dark:bg-gray-700 border-gray-300 dark:border-gray-600 checked:bg-blue-500 checked:border-blue-500"
                                    checked={!!appConfig.proxy.user_agent_override}
                                    onChange={(e) => {
                                        if (e.target.checked) {
                                            const restoredValue = appConfig.proxy.saved_user_agent || 'antigravity/1.15.8 darwin/arm64';
                                            updateProxyConfig({ user_agent_override: restoredValue, saved_user_agent: restoredValue });
                                        } else {
                                            updateProxyConfig({ user_agent_override: undefined });
                                        }
                                    }} />
                            </div>
                            {!!appConfig.proxy.user_agent_override && (
                                <div className="space-y-2 animate-in fade-in slide-in-from-top-1 duration-200">
                                    <input type="text" value={appConfig.proxy.user_agent_override}
                                        onChange={(e) => { const v = e.target.value; updateProxyConfig({ user_agent_override: v, saved_user_agent: v }); }}
                                        className="w-full px-2.5 py-1.5 border border-gray-300 dark:border-base-200 rounded-lg bg-white dark:bg-base-200 text-xs font-mono text-gray-900 dark:text-base-content focus:ring-2 focus:ring-blue-500 focus:border-transparent"
                                        placeholder={t('proxy.config.request.user_agent_placeholder', { defaultValue: 'Enter custom User-Agent string...' })} />
                                    <div className="bg-gray-50 dark:bg-base-300 rounded p-2 text-[10px] text-gray-500 font-mono break-all">
                                        <span className="font-bold select-none mr-2">{t('common.example', { defaultValue: 'Example' })}:</span>
                                        antigravity/1.15.8 darwin/arm64
                                    </div>
                                </div>
                            )}
                        </div>
                    </div>
                </div>

                {/* CLI 配置一键同步 */}
                <div className="bg-white dark:bg-base-100 rounded-xl shadow-sm border border-gray-100 dark:border-gray-700/50 overflow-hidden transition-all duration-200 hover:shadow-md">
                    <div className="px-5 py-4 flex items-center justify-between bg-gray-50/50 dark:bg-gray-800/50">
                        <div className="flex items-center gap-3">
                            <div className="text-gray-500 dark:text-gray-400"><Terminal size={18} /></div>
                            <span className="font-medium text-sm text-gray-900 dark:text-gray-100">{t('proxy.cli_sync.title', { defaultValue: 'CLI 配置一键同步' })}</span>
                        </div>
                    </div>
                    <div className="p-5">
                        <CliSyncCard
                            proxyUrl={status.running ? status.base_url : `http://127.0.0.1:${appConfig.proxy.port || 8150}`}
                            apiKey={appConfig.proxy.api_key}
                        />
                    </div>
                </div>

                {/* MCP 系统 */}
                <div className="bg-white dark:bg-base-100 rounded-xl shadow-sm border border-gray-100 dark:border-gray-700/50 overflow-hidden transition-all duration-200 hover:shadow-md">
                    <div className="px-5 py-4 flex items-center justify-between bg-gray-50/50 dark:bg-gray-800/50">
                        <div className="flex items-center gap-3">
                            <div className="text-blue-500"><Puzzle size={18} /></div>
                            <span className="font-medium text-sm text-gray-900 dark:text-gray-100">{t('proxy.config.zai.mcp.title')}</span>
                            <div className="flex gap-2 text-[10px]">
                                {['web_search', 'web_reader', 'vision'].map(f =>
                                    appConfig.proxy.zai?.mcp?.[(f + '_enabled') as keyof typeof appConfig.proxy.zai.mcp] && (
                                        <span key={f} className="bg-blue-500 dark:bg-blue-600 px-1.5 py-0.5 rounded text-white font-semibold shadow-sm">
                                            {t(`proxy.config.zai.mcp.${f}`).split(' ')[0]}
                                        </span>
                                    )
                                )}
                            </div>
                        </div>
                        <div onClick={(e) => e.stopPropagation()}>
                            <input type="checkbox" className="toggle toggle-sm bg-gray-200 dark:bg-gray-700 border-gray-300 dark:border-gray-600 checked:bg-blue-500 checked:border-blue-500"
                                checked={!!appConfig.proxy.zai?.mcp?.enabled}
                                onChange={(e) => updateZaiMcpConfig({ enabled: e.target.checked })} />
                        </div>
                    </div>
                    <div className="p-5 space-y-3">
                        <div className="grid grid-cols-2 md:grid-cols-3 gap-3">
                            <label className="flex items-center gap-2 border border-gray-100 dark:border-base-200 p-2 rounded-lg cursor-pointer hover:bg-gray-50 dark:hover:bg-base-200/50 transition-colors">
                                <input type="checkbox" className="checkbox checkbox-xs rounded border-2 border-gray-400 dark:border-gray-500 checked:border-blue-600 checked:bg-blue-600 [--chkbg:theme(colors.blue.600)] [--chkfg:white]"
                                    checked={!!appConfig.proxy.zai?.mcp?.web_search_enabled}
                                    onChange={(e) => updateZaiMcpConfig({ web_search_enabled: e.target.checked })} />
                                <span className="text-xs">{t('proxy.config.zai.mcp.web_search')}</span>
                            </label>
                            <label className="flex items-center gap-2 border border-gray-100 dark:border-base-200 p-2 rounded-lg cursor-pointer hover:bg-gray-50 dark:hover:bg-base-200/50 transition-colors">
                                <input type="checkbox" className="checkbox checkbox-xs rounded border-2 border-gray-400 dark:border-gray-500 checked:border-blue-600 checked:bg-blue-600 [--chkbg:theme(colors.blue.600)] [--chkfg:white]"
                                    checked={!!appConfig.proxy.zai?.mcp?.web_reader_enabled}
                                    onChange={(e) => updateZaiMcpConfig({ web_reader_enabled: e.target.checked })} />
                                <span className="text-xs">{t('proxy.config.zai.mcp.web_reader')}</span>
                            </label>
                            <label className="flex items-center gap-2 border border-gray-100 dark:border-base-200 p-2 rounded-lg cursor-pointer hover:bg-gray-50 dark:hover:bg-base-200/50 transition-colors">
                                <input type="checkbox" className="checkbox checkbox-xs rounded border-2 border-gray-400 dark:border-gray-500 checked:border-blue-600 checked:bg-blue-600 [--chkbg:theme(colors.blue.600)] [--chkfg:white]"
                                    checked={!!appConfig.proxy.zai?.mcp?.vision_enabled}
                                    onChange={(e) => updateZaiMcpConfig({ vision_enabled: e.target.checked })} />
                                <span className="text-xs">{t('proxy.config.zai.mcp.vision')}</span>
                            </label>
                        </div>
                        {appConfig.proxy.zai?.mcp?.enabled && (
                            <div className="bg-slate-100 dark:bg-slate-800/80 rounded-lg p-3 text-[10px] font-mono text-slate-600 dark:text-slate-400">
                                <div className="mb-1 font-bold text-gray-400 uppercase tracking-wider">{t('proxy.config.zai.mcp.local_endpoints')}</div>
                                <div className="space-y-0.5 select-all">
                                    {appConfig.proxy.zai?.mcp?.web_search_enabled && <div>http://127.0.0.1:{status.running ? status.port : (appConfig.proxy.port || 8150)}/mcp/web_search_prime/mcp</div>}
                                    {appConfig.proxy.zai?.mcp?.web_reader_enabled && <div>http://127.0.0.1:{status.running ? status.port : (appConfig.proxy.port || 8150)}/mcp/web_reader/mcp</div>}
                                    {appConfig.proxy.zai?.mcp?.vision_enabled && <div>http://127.0.0.1:{status.running ? status.port : (appConfig.proxy.port || 8150)}/mcp/zai-mcp-server/mcp</div>}
                                </div>
                            </div>
                        )}
                    </div>
                </div>

                {/* 多协议支持 */}
                <div className="bg-white dark:bg-base-100 rounded-xl shadow-sm border border-gray-100 dark:border-base-200 overflow-hidden">
                    <div className="p-3">
                        <div className="flex items-center gap-3 mb-3">
                            <div className="w-8 h-8 rounded-lg bg-gradient-to-br from-blue-500 to-purple-600 flex items-center justify-center shadow-md">
                                <Code size={16} className="text-white" />
                            </div>
                            <div>
                                <h3 className="text-base font-bold text-gray-900 dark:text-base-content">{t('proxy.multi_protocol.title')}</h3>
                                <p className="text-[10px] text-gray-500 dark:text-gray-400">{t('proxy.multi_protocol.subtitle')}</p>
                            </div>
                        </div>
                        <p className="text-xs text-gray-700 dark:text-gray-300 mb-4 leading-relaxed">{t('proxy.multi_protocol.description')}</p>
                        <div className="grid grid-cols-1 md:grid-cols-3 gap-3">
                            {/* OpenAI Card */}
                            <div className={cn('p-3 rounded-xl border-2 transition-all cursor-pointer', selectedProtocol === 'openai' ? 'border-blue-500 bg-blue-50/30 dark:bg-blue-900/10' : 'border-gray-100 dark:border-base-200 hover:border-blue-200')}
                                onClick={() => setSelectedProtocol('openai')}>
                                <div className="flex items-center justify-between mb-2">
                                    <span className="text-xs font-bold text-blue-600">{t('proxy.multi_protocol.openai_label')}</span>
                                    <button onClick={(e) => { e.stopPropagation(); copyToClipboardHandler(`http://127.0.0.1:${status.running ? status.port : (appConfig.proxy.port || 8150)}/v1`, 'openai'); }} className="btn btn-ghost btn-xs">
                                        {copied === 'openai' ? <CheckCircle size={14} /> : <div className="flex items-center gap-1 text-[10px] uppercase font-bold tracking-tighter"><Copy size={12} /> {t('proxy.multi_protocol.copy_base', { defaultValue: 'Base' })}</div>}
                                    </button>
                                </div>
                                <div className="space-y-1">
                                    <div className="flex items-center justify-between hover:bg-black/5 dark:hover:bg-white/5 rounded p-0.5 group">
                                        <code className="text-[10px] opacity-70">/v1/chat/completions</code>
                                        <button onClick={(e) => { e.stopPropagation(); copyToClipboardHandler(`http://127.0.0.1:${status.running ? status.port : (appConfig.proxy.port || 8150)}/v1/chat/completions`, 'openai-chat'); }} className="opacity-0 group-hover:opacity-100 transition-opacity">{copied === 'openai-chat' ? <CheckCircle size={10} className="text-green-500" /> : <Copy size={10} />}</button>
                                    </div>
                                    <div className="flex items-center justify-between hover:bg-black/5 dark:hover:bg-white/5 rounded p-0.5 group">
                                        <code className="text-[10px] opacity-70">/v1/completions</code>
                                        <button onClick={(e) => { e.stopPropagation(); copyToClipboardHandler(`http://127.0.0.1:${status.running ? status.port : (appConfig.proxy.port || 8150)}/v1/completions`, 'openai-compl'); }} className="opacity-0 group-hover:opacity-100 transition-opacity">{copied === 'openai-compl' ? <CheckCircle size={10} className="text-green-500" /> : <Copy size={10} />}</button>
                                    </div>
                                    <div className="flex items-center justify-between hover:bg-black/5 dark:hover:bg-white/5 rounded p-0.5 group">
                                        <code className="text-[10px] opacity-70 font-bold text-blue-500">/v1/responses (Codex)</code>
                                        <button onClick={(e) => { e.stopPropagation(); copyToClipboardHandler(`http://127.0.0.1:${status.running ? status.port : (appConfig.proxy.port || 8150)}/v1/responses`, 'openai-resp'); }} className="opacity-0 group-hover:opacity-100 transition-opacity">{copied === 'openai-resp' ? <CheckCircle size={10} className="text-green-500" /> : <Copy size={10} />}</button>
                                    </div>
                                </div>
                            </div>
                            {/* Anthropic Card */}
                            <div className={cn('p-3 rounded-xl border-2 transition-all cursor-pointer', selectedProtocol === 'anthropic' ? 'border-purple-500 bg-purple-50/30 dark:bg-purple-900/10' : 'border-gray-100 dark:border-base-200 hover:border-purple-200')}
                                onClick={() => setSelectedProtocol('anthropic')}>
                                <div className="flex items-center justify-between mb-2">
                                    <span className="text-xs font-bold text-purple-600">{t('proxy.multi_protocol.anthropic_label')}</span>
                                    <button onClick={(e) => { e.stopPropagation(); copyToClipboardHandler(`http://127.0.0.1:${status.running ? status.port : (appConfig.proxy.port || 8150)}/v1/messages`, 'anthropic'); }} className="btn btn-ghost btn-xs">
                                        {copied === 'anthropic' ? <CheckCircle size={14} /> : <Copy size={14} />}
                                    </button>
                                </div>
                                <code className="text-[10px] block truncate bg-black/5 dark:bg-white/5 p-1 rounded">/v1/messages</code>
                            </div>
                            {/* Gemini Card */}
                            <div className={cn('p-3 rounded-xl border-2 transition-all cursor-pointer', selectedProtocol === 'gemini' ? 'border-green-500 bg-green-50/30 dark:bg-green-900/10' : 'border-gray-100 dark:border-base-200 hover:border-green-200')}
                                onClick={() => setSelectedProtocol('gemini')}>
                                <div className="flex items-center justify-between mb-2">
                                    <span className="text-xs font-bold text-green-600">{t('proxy.multi_protocol.gemini_label')}</span>
                                    <button onClick={(e) => { e.stopPropagation(); copyToClipboardHandler(`http://127.0.0.1:${status.running ? status.port : (appConfig.proxy.port || 8150)}/v1beta/models`, 'gemini'); }} className="btn btn-ghost btn-xs">
                                        {copied === 'gemini' ? <CheckCircle size={14} /> : <Copy size={14} />}
                                    </button>
                                </div>
                                <code className="text-[10px] block truncate bg-black/5 dark:bg-white/5 p-1 rounded">/v1beta/models/...</code>
                            </div>
                        </div>
                    </div>
                </div>

                {/* 支持模型与集成 */}
                <div className="bg-white dark:bg-base-100 rounded-xl shadow-sm border border-gray-100 dark:border-base-200 overflow-hidden mt-4">
                    <div className="px-4 py-2.5 border-b border-gray-100 dark:border-base-200">
                        <h2 className="text-base font-bold text-gray-900 dark:text-base-content flex items-center gap-2">
                            <Terminal size={18} />
                            {t('proxy.supported_models.title')}
                        </h2>
                    </div>
                    <div className="p-4">
                        <div className="grid grid-cols-1 md:grid-cols-3 gap-4">
                            {/* 模型列表 */}
                            <div className="md:col-span-2">
                                <div className="mb-3">
                                    <GroupedSelect
                                        value={selectedModelId}
                                        onChange={(v) => setSelectedModelId(v)}
                                        options={customMappingOptions}
                                        placeholder={t('proxy.supported_models.select_model')}
                                    />
                                </div>
                                <div className="max-h-64 overflow-y-auto border border-gray-100 dark:border-base-200 rounded-lg">
                                    {filteredModels.map(model => (
                                        <div key={model.id}
                                            className={cn('flex items-center gap-3 p-2.5 cursor-pointer border-b border-gray-50 dark:border-base-200 last:border-b-0 transition-colors hover:bg-gray-50 dark:hover:bg-base-200/50',
                                                selectedModelId === model.id ? 'bg-blue-50 dark:bg-blue-900/20 border-l-2 border-l-blue-500' : '')}
                                            onClick={() => setSelectedModelId(model.id)}>
                                            <span className="text-lg">{model.icon}</span>
                                            <div className="flex-1 min-w-0">
                                                <div className="text-xs font-semibold text-gray-900 dark:text-base-content truncate">{model.name}</div>
                                                <div className="text-[10px] text-gray-500 dark:text-gray-400 font-mono">{model.id}</div>
                                            </div>
                                            <button onClick={(e) => { e.stopPropagation(); copyToClipboardHandler(model.id, model.id); }}
                                                className="p-1 rounded hover:bg-gray-200 dark:hover:bg-base-300 transition-colors">
                                                {copied === model.id ? <CheckCircle size={12} className="text-green-500" /> : <Copy size={12} />}
                                            </button>
                                        </div>
                                    ))}
                                </div>
                            </div>
                            {/* 代码预览 */}
                            <div className="md:col-span-1">
                                <div className="bg-gray-900 dark:bg-base-300 rounded-lg p-3 text-[10px] font-mono text-gray-100 dark:text-base-content overflow-x-auto whitespace-pre-wrap min-h-[300px]">
                                    {getPythonExample(selectedModelId)}
                                </div>
                            </div>
                        </div>
                    </div>
                </div>

                {/* 公网访问 (Cloudflared) - 仅在桌面端显示 */}
                {isTauri() && (
                    <div className="bg-white dark:bg-base-100 rounded-xl shadow-sm border border-gray-100 dark:border-base-200 overflow-hidden mt-4">
                        <div className="px-5 py-4 flex items-center justify-between bg-gray-50/50 dark:bg-gray-800/50">
                            <div className="flex items-center gap-3">
                                <svg xmlns="http://www.w3.org/2000/svg" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" className="text-orange-500"><path d="M12 2L2 7l10 5 10-5-10-5z" /><path d="M2 17l10 5 10-5" /><path d="M2 12l10 5 10-5" /></svg>
                                <span className="font-medium text-sm text-gray-900 dark:text-gray-100">{t('proxy.cloudflared.title', { defaultValue: 'Public Access (Cloudflared)' })}</span>
                                {cfStatus.running && (
                                    <div className="text-xs px-2 py-0.5 rounded-full bg-green-100 text-green-700 dark:bg-green-900/40 dark:text-green-400">{t('common.enabled')}</div>
                                )}
                            </div>
                            <div className="flex items-center gap-4">
                                {cfLoading ? (
                                    <span className="loading loading-spinner loading-xs"></span>
                                ) : cfStatus.running && cfStatus.url ? (
                                    <button
                                        onClick={handleCfCopyUrl}
                                        className="text-xs px-2 py-1 rounded bg-green-100 dark:bg-green-900/30 text-green-700 dark:text-green-400 hover:bg-green-200 dark:hover:bg-green-900/50 transition-colors flex items-center gap-1"
                                    >
                                        {copied === 'cf-url' ? <CheckCircle size={12} /> : <Copy size={12} />}
                                        {cfStatus.url.replace('https://', '').slice(0, 20)}...
                                    </button>
                                ) : null}
                                <div onClick={(e) => e.stopPropagation()}>
                                    <input
                                        type="checkbox"
                                        className="toggle toggle-sm bg-gray-200 dark:bg-gray-700 border-gray-300 dark:border-gray-600 checked:bg-orange-500 checked:border-orange-500"
                                        checked={cfStatus.running}
                                        onChange={(e) => handleCfToggle(e.target.checked)}
                                    />
                                </div>
                            </div>
                        </div>
                        <div className="p-5 space-y-4">
                            {!cfStatus.installed ? (
                                <div className="flex items-center justify-between p-4 bg-yellow-50 dark:bg-yellow-900/20 rounded-xl border border-yellow-200 dark:border-yellow-800">
                                    <div className="space-y-1">
                                        <span className="text-sm font-bold text-yellow-800 dark:text-yellow-200">
                                            {t('proxy.cloudflared.not_installed', { defaultValue: 'Cloudflared not installed' })}
                                        </span>
                                        <p className="text-xs text-yellow-600 dark:text-yellow-400">
                                            {t('proxy.cloudflared.install_hint', { defaultValue: 'Click to download and install cloudflared binary' })}
                                        </p>
                                    </div>
                                    <button
                                        onClick={handleCfInstall}
                                        disabled={cfLoading}
                                        className="px-4 py-2 rounded-lg text-sm font-medium bg-yellow-500 text-white hover:bg-yellow-600 disabled:opacity-50 flex items-center gap-2"
                                    >
                                        {cfLoading ? <span className="loading loading-spinner loading-xs"></span> : null}
                                        {t('proxy.cloudflared.install', { defaultValue: 'Install' })}
                                    </button>
                                </div>
                            ) : (
                                <>
                                    <div className="flex items-center gap-2 text-xs text-gray-500 dark:text-gray-400">
                                        <CheckCircle size={14} className="text-green-500" />
                                        {t('proxy.cloudflared.installed', { defaultValue: 'Installed' })}: {cfStatus.version || 'Unknown'}
                                    </div>
                                    <div className="grid grid-cols-2 gap-3">
                                        <button
                                            onClick={() => {
                                                setCfMode('quick');
                                                if (appConfig) {
                                                    saveConfig({ ...appConfig, cloudflared: { ...appConfig.cloudflared, mode: 'quick' } });
                                                }
                                            }}
                                            disabled={cfStatus.running}
                                            className={cn(
                                                "p-3 rounded-lg border-2 text-left transition-all",
                                                cfMode === 'quick' ? "border-orange-500 bg-orange-50 dark:bg-orange-900/20" : "border-gray-200 dark:border-gray-700 hover:border-gray-300 dark:hover:border-gray-600",
                                                cfStatus.running && "opacity-60 cursor-not-allowed"
                                            )}
                                        >
                                            <div className="text-sm font-bold text-gray-900 dark:text-base-content">
                                                {t('proxy.cloudflared.mode_quick', { defaultValue: 'Quick Tunnel' })}
                                            </div>
                                            <p className="text-[10px] text-gray-500 dark:text-gray-400 mt-1">
                                                {t('proxy.cloudflared.mode_quick_desc', { defaultValue: 'Auto-generated temporary URL (*.trycloudflare.com)' })}
                                            </p>
                                        </button>
                                        <button
                                            onClick={() => {
                                                setCfMode('auth');
                                                if (appConfig) {
                                                    saveConfig({ ...appConfig, cloudflared: { ...appConfig.cloudflared, mode: 'auth' } });
                                                }
                                            }}
                                            disabled={cfStatus.running}
                                            className={cn(
                                                "p-3 rounded-lg border-2 text-left transition-all",
                                                cfMode === 'auth' ? "border-orange-500 bg-orange-50 dark:bg-orange-900/20" : "border-gray-200 dark:border-gray-700 hover:border-gray-300 dark:hover:border-gray-600",
                                                cfStatus.running && "opacity-60 cursor-not-allowed"
                                            )}
                                        >
                                            <div className="text-sm font-bold text-gray-900 dark:text-base-content">
                                                {t('proxy.cloudflared.mode_auth', { defaultValue: 'Named Tunnel' })}
                                            </div>
                                            <p className="text-[10px] text-gray-500 dark:text-gray-400 mt-1">
                                                {t('proxy.cloudflared.mode_auth_desc', { defaultValue: 'Use your Cloudflare account with custom domain' })}
                                            </p>
                                        </button>
                                    </div>
                                    {cfMode === 'auth' && (
                                        <div className="space-y-2">
                                            <label className="text-sm font-medium text-gray-700 dark:text-gray-300">
                                                {t('proxy.cloudflared.token', { defaultValue: 'Tunnel Token' })}
                                            </label>
                                            <input
                                                type="password"
                                                value={cfToken}
                                                onChange={(e) => setCfToken(e.target.value)}
                                                onBlur={() => {
                                                    if (appConfig) {
                                                        saveConfig({ ...appConfig, cloudflared: { ...appConfig.cloudflared, token: cfToken } });
                                                    }
                                                }}
                                                disabled={cfStatus.running}
                                                placeholder="eyJhIjoiNj..."
                                                className="w-full px-3 py-2 rounded-lg border border-gray-200 dark:border-gray-700 bg-white dark:bg-base-200 text-sm font-mono disabled:opacity-60"
                                            />
                                        </div>
                                    )}
                                    <div className="flex items-center justify-between p-3 bg-gray-50 dark:bg-base-200 rounded-lg">
                                        <div className="space-y-0.5">
                                            <span className="text-sm font-medium text-gray-900 dark:text-base-content">
                                                {t('proxy.cloudflared.use_http2', { defaultValue: 'Use HTTP/2' })}
                                            </span>
                                            <p className="text-[10px] text-gray-500 dark:text-gray-400">
                                                {t('proxy.cloudflared.use_http2_desc', { defaultValue: 'More compatible, recommended for China mainland' })}
                                            </p>
                                        </div>
                                        <input
                                            type="checkbox"
                                            className="toggle toggle-sm"
                                            checked={cfUseHttp2}
                                            onChange={(e) => {
                                                const val = e.target.checked;
                                                setCfUseHttp2(val);
                                                if (appConfig) {
                                                    saveConfig({
                                                        ...appConfig,
                                                        cloudflared: { ...appConfig.cloudflared, use_http2: val }
                                                    });
                                                }
                                            }}
                                            disabled={cfStatus.running}
                                        />
                                    </div>
                                    {cfStatus.running && (
                                        <div className="p-4 bg-green-50 dark:bg-green-900/20 rounded-xl border border-green-200 dark:border-green-800">
                                            <div className="flex items-center gap-2 mb-2">
                                                <div className="w-2 h-2 rounded-full bg-green-500 animate-pulse"></div>
                                                <span className="text-sm font-bold text-green-800 dark:text-green-200">
                                                    {t('proxy.cloudflared.running', { defaultValue: 'Tunnel Running' })}
                                                </span>
                                            </div>
                                            {cfStatus.url && (
                                                <div className="flex items-center gap-2">
                                                    <code className="flex-1 px-3 py-2 bg-white dark:bg-base-100 rounded text-xs font-mono text-gray-800 dark:text-gray-200 border border-green-200 dark:border-green-800">
                                                        {cfStatus.url}
                                                    </code>
                                                    <button
                                                        onClick={handleCfCopyUrl}
                                                        className="p-2 rounded-lg bg-green-500 text-white hover:bg-green-600 transition-colors"
                                                    >
                                                        {copied === 'cf-url' ? <CheckCircle size={16} /> : <Copy size={16} />}
                                                    </button>
                                                </div>
                                            )}
                                        </div>
                                    )}
                                    {cfStatus.error && (
                                        <div className="p-3 bg-red-50 dark:bg-red-900/20 rounded-lg border border-red-200 dark:border-red-800 text-sm text-red-700 dark:text-red-300">
                                            {cfStatus.error}
                                        </div>
                                    )}
                                </>
                            )}
                        </div>
                    </div>
                )}
            </div>
        </div>
    );
}
