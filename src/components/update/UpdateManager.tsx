import { useEffect } from 'react';
import { getVersion } from '@tauri-apps/api/app';
import { relaunch } from '@tauri-apps/plugin-process';
import { check } from '@tauri-apps/plugin-updater';
import { useTranslation } from 'react-i18next';

import { isTauri } from '../../utils/env';

export default function UpdateManager() {
  const { t } = useTranslation();

  useEffect(() => {
    if (!isTauri()) return;

    let cancelled = false;
    const checkAndInstall = async (): Promise<void> => {
      try {
        const [currentVersion, update] = await Promise.all([getVersion(), check()]);
        if (!update || cancelled) return;

        const accepted = window.confirm(
          t('update_notification.message', {
            current: currentVersion,
            defaultValue: `Version ${update.version} is ready. Download and install it now?`,
          }),
        );
        if (!accepted || cancelled) return;

        await update.downloadAndInstall();
        if (!cancelled) await relaunch();
      } catch (error: unknown) {
        console.error('[Updater] Update check or installation failed:', error);
      }
    };

    void checkAndInstall();
    return () => {
      cancelled = true;
    };
  }, [t]);

  return null;
}
