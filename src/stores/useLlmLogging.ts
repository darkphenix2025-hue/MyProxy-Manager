import { create } from 'zustand';
import { request as invoke } from '../utils/request';

interface LlmLoggingState {
    enabled: boolean;
    initialized: boolean;

    setEnabled: (value: boolean) => void;
    loadStatus: () => Promise<void>;
    toggle: (enabled: boolean) => Promise<boolean>;
}

export const useLlmLogging = create<LlmLoggingState>((set) => ({
    enabled: false,
    initialized: false,

    setEnabled: (value: boolean) => set({ enabled: value }),

    loadStatus: async () => {
        try {
            const status = await invoke<{ enabled: boolean }>('get_llm_logging_status');
            set({ enabled: status.enabled, initialized: true });
        } catch (err) {
            console.error('Failed to load LLM logging status:', err);
            set({ initialized: true });
        }
    },

    toggle: async (enabled: boolean) => {
        try {
            if (enabled) {
                await invoke('enable_llm_logging');
            } else {
                await invoke('disable_llm_logging');
            }
            set({ enabled });
            return true;
        } catch (err) {
            console.error('Failed to toggle LLM logging:', err);
            return false;
        }
    },
}));
