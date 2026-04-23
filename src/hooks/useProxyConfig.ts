import { useState, useEffect } from 'react';
import { AppConfig } from '../types/config';
import { showToast } from '../components/common/ToastContainer';
import { request as invoke } from '../utils/request';

interface ProxyStatus {
    running: boolean;
    port: number;
    base_url: string;
    active_accounts: number;
}

interface UseProxyConfigReturn {
    appConfig: AppConfig | null;
    configLoading: boolean;
    configError: string | null;
    status: ProxyStatus;
    loading: boolean;
    setAppConfig: React.Dispatch<React.SetStateAction<AppConfig | null>>;
    saveConfig: (newConfig: AppConfig) => Promise<void>;
    handleToggle: () => Promise<void>;
    loadConfig: () => Promise<void>;
    loadStatus: () => Promise<void>;
}

export function useProxyConfig(): UseProxyConfigReturn {
    const [appConfig, setAppConfig] = useState<AppConfig | null>(null);
    const [configLoading, setConfigLoading] = useState(true);
    const [configError, setConfigError] = useState<string | null>(null);
    const [status, setStatus] = useState<ProxyStatus>({
        running: false,
        port: 0,
        base_url: '',
        active_accounts: 0,
    });
    const [loading, setLoading] = useState(false);

    const loadConfig = async () => {
        setConfigLoading(true);
        setConfigError(null);
        try {
            const config = await invoke<AppConfig>('load_config');
            setAppConfig(config);
        } catch (error) {
            console.error('加载配置失败:', error);
            setConfigError(String(error));
        } finally {
            setConfigLoading(false);
        }
    };

    const loadStatus = async () => {
        try {
            const s = await invoke<ProxyStatus>('get_proxy_status');
            if (s.base_url === 'starting' || s.base_url === 'busy') {
                setStatus(prev => ({ ...s, running: prev.running }));
            } else {
                setStatus(s);
            }
        } catch (error) {
            console.error('获取状态失败:', error);
        }
    };

    const saveConfig = async (newConfig: AppConfig) => {
        setAppConfig(newConfig);
        try {
            await invoke('save_config', { config: newConfig });
        } catch (error) {
            console.error('保存配置失败:', error);
            showToast(`保存失败: ${error}`, 'error');
        }
    };

    const handleToggle = async () => {
        if (!appConfig) return;
        setLoading(true);
        try {
            if (status.running) {
                await invoke('stop_proxy_service');
                showToast('代理服务已停止', 'success');
            } else {
                await invoke('start_proxy_service', { config: appConfig.proxy });
                showToast('代理服务已启动', 'success');
            }
            await loadStatus();
        } catch (error) {
            console.error('切换服务状态失败:', error);
            showToast(`操作失败: ${error}`, 'error');
        } finally {
            setLoading(false);
        }
    };

    useEffect(() => {
        loadConfig();
        loadStatus();
        const interval = setInterval(loadStatus, 3000);
        return () => clearInterval(interval);
    }, []);

    return {
        appConfig,
        configLoading,
        configError,
        status,
        loading,
        setAppConfig,
        saveConfig,
        handleToggle,
        loadConfig,
        loadStatus,
    };
}
