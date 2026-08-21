import {
  CheckCircle2,
  CircleAlert,
  Clock3,
  Download,
  FileKey2,
  Gauge,
  KeyRound,
  Loader2,
  List,
  Power,
  RefreshCw,
  ShieldCheck,
  TimerReset,
  Trash2,
  Upload,
} from 'lucide-react';
import { open, save } from '@tauri-apps/plugin-dialog';
import { invoke as tauriInvoke } from '@tauri-apps/api/core';
import { useCallback, useEffect, useRef, useState } from 'react';
import type { ChangeEvent, ReactElement } from 'react';
import { useTranslation } from 'react-i18next';
import { showToast } from '../common/ToastContainer';
import { request } from '../../utils/request';
import { isTauri } from '../../utils/env';
import {
  codexModelCatalogKey,
  getModelCatalog,
  removeModelCatalog,
  setModelCatalog,
} from '../../services/modelCatalogCache';
import type {
  CodexConnection,
  CodexLoginStart,
  CodexLoginStatus,
  CodexModel,
  CodexQuotaLimit,
  CodexQuotaSummary,
  CodexQuotaWindow,
} from '../../types/codex';

interface CodexOAuthPanelProps {
  readonly onConnectionsChange?: (connections: ReadonlyArray<CodexConnection>) => void;
}

type LifecycleStatus = 'ready' | 'pending' | 'expired' | 'revoked' | 'unavailable' | 'disabled';
type ActionName = 'toggle' | 'refresh' | 'models' | 'quota' | 'export' | 'delete';

function formatError(error: unknown): string {
  if (error instanceof Error) return error.message;
  if (typeof error === 'string') return error;
  if (error && typeof error === 'object') {
    try {
      return JSON.stringify(error);
    } catch {
      return String(error);
    }
  }
  return String(error);
}

function formatExpiry(expiresAt?: number): string {
  if (!expiresAt) return '—';
  const date = new Date(expiresAt * 1000);
  return Number.isNaN(date.getTime()) ? '—' : date.toLocaleString();
}

function formatQuotaDuration(seconds?: number): string {
  if (!seconds || seconds < 0) return '—';
  if (seconds >= 86_400) return `${Math.floor(seconds / 86_400)}天`;
  if (seconds >= 3_600) return `${Math.floor(seconds / 3_600)}小时`;
  if (seconds >= 60) return `${Math.floor(seconds / 60)}分钟`;
  return `${seconds}秒`;
}

function quotaRemainingPercent(window?: CodexQuotaWindow): number | null {
  if (!window || typeof window.used_percent !== 'number' || !Number.isFinite(window.used_percent)) {
    return null;
  }
  const usedPercent = Math.min(100, Math.max(0, window.used_percent));
  return 100 - usedPercent;
}

function QuotaWindowCard({
  label,
  window,
  resetLabel,
  remainingLabel,
}: {
  readonly label: string;
  readonly window?: CodexQuotaWindow;
  readonly resetLabel: string;
  readonly remainingLabel: string;
}): ReactElement | null {
  if (!window) return null;
  const remainingPercent = quotaRemainingPercent(window);
  const progressClass = remainingPercent !== null && remainingPercent <= 10
    ? 'bg-rose-500'
    : remainingPercent !== null && remainingPercent <= 25
      ? 'bg-amber-500'
      : 'bg-emerald-500';
  return (
    <div className="rounded-lg border border-slate-200 bg-white px-3 py-2.5 dark:border-slate-700 dark:bg-slate-900/70">
      <div className="flex items-center justify-between gap-2 text-[11px]">
        <span className="font-semibold text-slate-700 dark:text-slate-200">{label}</span>
        <span className="font-bold text-slate-900 dark:text-white">
          {remainingLabel} {remainingPercent === null ? '—' : `${remainingPercent.toFixed(0)}%`}
        </span>
      </div>
      <div className="mt-2 h-1.5 overflow-hidden rounded-full bg-slate-200 dark:bg-slate-700">
        <div className={`h-full rounded-full transition-all ${progressClass}`} style={{ width: `${remainingPercent ?? 0}%` }} />
      </div>
      <div className="mt-2 flex flex-wrap gap-x-3 gap-y-1 text-[10px] font-medium text-slate-500 dark:text-slate-400">
        {window.reset_at ? <span>{resetLabel} {formatExpiry(window.reset_at)}</span> : null}
        {window.reset_after_seconds ? <span>约 {formatQuotaDuration(window.reset_after_seconds)}后</span> : null}
      </div>
    </div>
  );
}

function quotaWindows(limit?: CodexQuotaLimit): ReadonlyArray<{ label: string; window?: CodexQuotaWindow }> {
  if (!limit) return [];
  return [
    { label: 'Primary', window: limit.primary_window },
    { label: 'Secondary', window: limit.secondary_window },
  ].filter((item) => item.window);
}

function normalizeLifecycle(lifecycle: string): LifecycleStatus {
  switch (lifecycle.trim().toLowerCase()) {
    case 'ready':
      return 'ready';
    case 'pending_validation':
      return 'pending';
    case 'expired':
      return 'expired';
    case 'revoked':
      return 'revoked';
    default:
      return 'unavailable';
  }
}

function metadataText(value: unknown): string | null {
  return typeof value === 'string' && value.trim() ? value : null;
}

function connectionFileName(connection: CodexConnection): string {
  const configured = metadataText(connection.connection.config?.auth_file_name);
  return configured || 'codex-auth-file.json';
}

function connectionPlan(connection: CodexConnection): string | null {
  return metadataText(connection.identity.metadata?.plan);
}

const lifecycleClasses: Record<LifecycleStatus, string> = {
  ready:
    'border-emerald-200 bg-emerald-50 text-emerald-800 dark:border-emerald-800/70 dark:bg-emerald-950/60 dark:text-emerald-200',
  pending:
    'border-amber-200 bg-amber-50 text-amber-800 dark:border-amber-800/70 dark:bg-amber-950/60 dark:text-amber-200',
  expired:
    'border-rose-200 bg-rose-50 text-rose-800 dark:border-rose-800/70 dark:bg-rose-950/60 dark:text-rose-200',
  revoked:
    'border-rose-200 bg-rose-50 text-rose-800 dark:border-rose-800/70 dark:bg-rose-950/60 dark:text-rose-200',
  unavailable:
    'border-slate-300 bg-slate-100 text-slate-800 dark:border-slate-700 dark:bg-slate-800 dark:text-slate-200',
  disabled:
    'border-slate-300 bg-slate-100 text-slate-700 dark:border-slate-700 dark:bg-slate-800 dark:text-slate-300',
};

export default function CodexOAuthPanel({
  onConnectionsChange,
}: CodexOAuthPanelProps): ReactElement {
  const { t } = useTranslation();
  const [connections, setConnections] = useState<ReadonlyArray<CodexConnection>>([]);
  const [isLoading, setIsLoading] = useState(false);
  const [isLoggingIn, setIsLoggingIn] = useState(false);
  const [isImporting, setIsImporting] = useState(false);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [actionByCredential, setActionByCredential] = useState<Record<string, ActionName | undefined>>({});
  const [modelsByCredential, setModelsByCredential] = useState<Record<string, ReadonlyArray<CodexModel>>>({});
  const [quotaByCredential, setQuotaByCredential] = useState<Record<string, CodexQuotaSummary | undefined>>({});
  const [quotaErrorsByCredential, setQuotaErrorsByCredential] = useState<Record<string, string | undefined>>({});
  const [expandedModels, setExpandedModels] = useState<string | null>(null);
  const fileInputRef = useRef<HTMLInputElement>(null);

  const loadConnections = useCallback(async (): Promise<void> => {
    setIsLoading(true);
    try {
      const response = await request<CodexConnection[]>('list_account_connections');
      const next: ReadonlyArray<CodexConnection> = Array.isArray(response) ? response : [];
      setConnections(next);
      onConnectionsChange?.(next);
      setLoadError(null);
    } catch (error: unknown) {
      setLoadError(formatError(error));
    } finally {
      setIsLoading(false);
    }
  }, [onConnectionsChange]);

  const handleQuota = useCallback(
    async (connection: CodexConnection, notify = true): Promise<void> => {
      const credentialId = connection.credential.id;
      setActionByCredential((current) => ({ ...current, [credentialId]: 'quota' }));
      try {
        const quota = await request<CodexQuotaSummary>('get_codex_quota', {
          credential_id: credentialId,
        });
        setQuotaByCredential((current) => ({ ...current, [credentialId]: quota }));
        setQuotaErrorsByCredential((current) => ({ ...current, [credentialId]: undefined }));
        if (notify) showToast(t('accounts.codex_quota_refresh_success', 'Codex 配额已刷新'), 'success');
      } catch (error: unknown) {
        const message = formatError(error);
        setQuotaErrorsByCredential((current) => ({ ...current, [credentialId]: message }));
        if (notify) showToast(message, 'error');
      } finally {
        setActionByCredential((current) => ({ ...current, [credentialId]: undefined }));
      }
    },
    [t],
  );

  useEffect(() => {
    void loadConnections();
  }, [loadConnections]);

  useEffect(() => {
    setModelsByCredential((current) => {
      const next = { ...current };
      let changed = false;
      for (const connection of connections) {
        const credentialId = connection.credential.id;
        if (next[credentialId]) continue;
        const cached = getModelCatalog(codexModelCatalogKey(credentialId));
        if (!cached) continue;
        next[credentialId] = cached.models.map((model) => ({
          id: model.id,
          ...(model.displayName ? { display_name: model.displayName } : {}),
          ...(model.ownedBy ? { owned_by: model.ownedBy } : {}),
        }));
        changed = true;
      }
      return changed ? next : current;
    });
  }, [connections]);

  useEffect(() => {
    for (const connection of connections) {
      const credentialId = connection.credential.id;
      if (
        quotaByCredential[credentialId] === undefined &&
        quotaErrorsByCredential[credentialId] === undefined &&
        actionByCredential[credentialId] === undefined
      ) {
        void handleQuota(connection, false);
      }
    }
  }, [actionByCredential, connections, handleQuota, quotaByCredential, quotaErrorsByCredential]);

  const runAction = useCallback(
    async (
      credentialId: string,
      action: ActionName,
      operation: () => Promise<unknown>,
      successMessage: string,
    ): Promise<void> => {
      setActionByCredential((current) => ({ ...current, [credentialId]: action }));
      try {
        await operation();
        await loadConnections();
        showToast(successMessage, 'success');
      } catch (error: unknown) {
        showToast(formatError(error), 'error');
      } finally {
        setActionByCredential((current) => ({ ...current, [credentialId]: undefined }));
      }
    },
    [loadConnections],
  );

  const handleLogin = useCallback(async (): Promise<void> => {
    setIsLoggingIn(true);
    try {
      const start = await request<CodexLoginStart>('start_codex_login');
      if (!isTauri() && start.authorization_url) {
        window.open(start.authorization_url, '_blank', 'noopener,noreferrer');
      }

      let status: CodexLoginStatus = { phase: 'pending' };
      while (Date.now() < start.expires_at * 1000) {
        await new Promise<void>((resolve) => window.setTimeout(resolve, 1000));
        status = await request<CodexLoginStatus>('get_codex_login_status', {
          session_id: start.session_id,
        });
        if (status.phase !== 'pending') break;
      }

      if (status.phase === 'completed') {
        await loadConnections();
        showToast(t('accounts.codex_login_success', 'Codex OAuth 登录成功'), 'success');
      } else if (status.phase === 'cancelled') {
        showToast(t('accounts.codex_login_cancelled', 'Codex OAuth 登录已取消'), 'warning');
      } else {
        throw new Error(status.error_code || t('accounts.codex_login_failed', 'Codex OAuth 登录失败'));
      }
    } catch (error: unknown) {
      showToast(formatError(error), 'error');
    } finally {
      setIsLoggingIn(false);
    }
  }, [loadConnections, t]);

  const handleImportContent = useCallback(
    async (content: string, filename?: string): Promise<void> => {
      await request<CodexConnection>('import_codex_auth_file', { content, filename });
    },
    [],
  );

  const handleImport = useCallback(async (): Promise<void> => {
    if (!isTauri()) {
      fileInputRef.current?.click();
      return;
    }
    setIsImporting(true);
    try {
      const selected = await open({
        multiple: true,
        filters: [{ name: 'Codex auth-file', extensions: ['json'] }],
      });
      if (!selected) return;
      const paths = Array.isArray(selected) ? selected : [selected];
      let imported = 0;
      for (const path of paths) {
        const content = await tauriInvoke<string>('read_text_file', { path });
        const filename = path.split(/[\\/]/).pop();
        await handleImportContent(content, filename);
        imported += 1;
      }
      await loadConnections();
      showToast(
        t('accounts.codex_import_success', '已导入 {{count}} 个 Codex auth-file', { count: imported }),
        'success',
      );
    } catch (error: unknown) {
      showToast(formatError(error), 'error');
    } finally {
      setIsImporting(false);
    }
  }, [handleImportContent, loadConnections, t]);

  const handleWebFileChange = useCallback(
    async (event: ChangeEvent<HTMLInputElement>): Promise<void> => {
      const files = Array.from(event.target.files ?? []);
      event.target.value = '';
      if (files.length === 0) return;
      setIsImporting(true);
      try {
        for (const file of files) {
          await handleImportContent(await file.text(), file.name);
        }
        await loadConnections();
        showToast(
          t('accounts.codex_import_success', '已导入 {{count}} 个 Codex auth-file', { count: files.length }),
          'success',
        );
      } catch (error: unknown) {
        showToast(formatError(error), 'error');
      } finally {
        setIsImporting(false);
      }
    },
    [handleImportContent, loadConnections, t],
  );

  const handleToggle = useCallback(
    (connection: CodexConnection): void => {
      const enabled = !connection.connection.enabled;
      void runAction(
        connection.credential.id,
        'toggle',
        () =>
          request('set_codex_connection_enabled', {
            credential_id: connection.credential.id,
            enabled,
          }),
        enabled
          ? t('accounts.codex_enabled_success', 'Codex auth-file 已启用')
          : t('accounts.codex_disabled_success', 'Codex auth-file 已停用'),
      );
    },
    [runAction, t],
  );

  const handleRefresh = useCallback(
    (connection: CodexConnection): void => {
      const credentialId = connection.credential.id;
      void runAction(
        credentialId,
        'refresh',
        async () => {
          setModelsByCredential((current) => {
            const next = { ...current };
            delete next[credentialId];
            return next;
          });
          setExpandedModels((current) => (current === credentialId ? null : current));
          setQuotaByCredential((current) => ({ ...current, [credentialId]: undefined }));
          setQuotaErrorsByCredential((current) => ({ ...current, [credentialId]: undefined }));
          await request('refresh_codex_auth_file', { credential_id: credentialId });
        },
        t('accounts.codex_refresh_success', 'Codex OAuth 凭证已刷新'),
      );
    },
    [runAction, t],
  );

  const handleModels = useCallback(
    async (connection: CodexConnection): Promise<void> => {
      const credentialId = connection.credential.id;
      setActionByCredential((current) => ({ ...current, [credentialId]: 'models' }));
      try {
        const models = await request<CodexModel[]>('list_codex_models', {
          credential_id: credentialId,
        });
        setModelCatalog(codexModelCatalogKey(credentialId), models);
        setModelsByCredential((current) => ({ ...current, [credentialId]: models }));
        setExpandedModels(credentialId);
      } catch (error: unknown) {
        showToast(formatError(error), 'error');
      } finally {
        setActionByCredential((current) => ({ ...current, [credentialId]: undefined }));
      }
    },
    [],
  );

  const handleExport = useCallback(
    async (connection: CodexConnection): Promise<void> => {
      if (!isTauri()) {
        showToast(t('accounts.codex_export_desktop_only', '请在桌面应用中导出 auth-file'), 'warning');
        return;
      }
      const credentialId = connection.credential.id;
      setActionByCredential((current) => ({ ...current, [credentialId]: 'export' }));
      try {
        const path = await save({
          defaultPath: connectionFileName(connection),
          filters: [{ name: 'Codex auth-file', extensions: ['json'] }],
        });
        if (!path) return;
        await request('export_codex_auth_file', { credential_id: credentialId, path });
        showToast(t('accounts.codex_export_success', 'Codex auth-file 已导出'), 'success');
      } catch (error: unknown) {
        showToast(formatError(error), 'error');
      } finally {
        setActionByCredential((current) => ({ ...current, [credentialId]: undefined }));
      }
    },
    [t],
  );

  const handleDelete = useCallback(
    (connection: CodexConnection): void => {
      const identity = connection.identity.email || connection.identity.display_name || connection.credential.fingerprint;
      if (!window.confirm(t('accounts.codex_delete_confirm', '确定删除 {{identity}} 的 Codex auth-file 吗？', { identity }))) {
        return;
      }
      void runAction(
        connection.credential.id,
        'delete',
        async () => {
          await request('delete_codex_auth_file', { credential_id: connection.credential.id });
          removeModelCatalog(codexModelCatalogKey(connection.credential.id));
          setModelsByCredential((current) => {
            const next = { ...current };
            delete next[connection.credential.id];
            return next;
          });
          setQuotaByCredential((current) => {
            const next = { ...current };
            delete next[connection.credential.id];
            return next;
          });
          setQuotaErrorsByCredential((current) => {
            const next = { ...current };
            delete next[connection.credential.id];
            return next;
          });
          setExpandedModels((current) => (current === connection.credential.id ? null : current));
        },
        t('accounts.codex_delete_success', 'Codex auth-file 已删除'),
      );
    },
    [runAction, t],
  );

  const enabledCount = connections.filter((connection) => connection.connection.enabled).length;

  return (
    <section className="flex-none overflow-hidden rounded-2xl border border-indigo-100 bg-gradient-to-r from-indigo-50 via-white to-sky-50 shadow-sm dark:border-indigo-900/50 dark:from-indigo-950/40 dark:via-base-100 dark:to-sky-950/30">
      <input
        ref={fileInputRef}
        type="file"
        accept=".json,application/json"
        multiple
        className="hidden"
        onChange={(event) => void handleWebFileChange(event)}
      />
      <div className="flex flex-wrap items-center justify-between gap-3 border-b border-indigo-100/80 px-4 py-3 dark:border-indigo-900/40">
        <div className="flex min-w-0 items-center gap-3">
          <div className="flex h-9 w-9 shrink-0 items-center justify-center rounded-xl bg-indigo-600 text-white shadow-sm dark:bg-indigo-500">
            <ShieldCheck className="h-4 w-4" />
          </div>
          <div className="min-w-0">
            <h2 className="truncate text-[15px] font-semibold text-slate-950 dark:text-white">
              {t('accounts.codex_title', 'Codex 账号')}
            </h2>
            <p className="truncate text-xs font-medium text-slate-600 dark:text-slate-300">
              {t('accounts.codex_subtitle', '通过 OAuth 登录生成 auth-file，供供应商路由使用')}
            </p>
          </div>
        </div>
        <div className="flex shrink-0 flex-wrap items-center justify-end gap-2">
          <button
            type="button"
            className="inline-flex min-h-9 items-center gap-1.5 rounded-lg border border-indigo-200 bg-white px-3 text-xs font-medium text-indigo-700 transition-colors hover:bg-indigo-50 disabled:cursor-not-allowed disabled:opacity-60 dark:border-indigo-800 dark:bg-base-100 dark:text-indigo-300 dark:hover:bg-indigo-950/50"
            onClick={() => void handleImport()}
            disabled={isImporting || isLoggingIn}
          >
            {isImporting ? <Loader2 className="h-3.5 w-3.5 animate-spin" /> : <Upload className="h-3.5 w-3.5" />}
            {t('accounts.codex_import', '导入 auth-file')}
          </button>
          <button
            type="button"
            className="inline-flex min-h-9 items-center gap-1.5 rounded-lg border border-indigo-200 bg-white px-3 text-xs font-medium text-indigo-700 transition-colors hover:bg-indigo-50 disabled:cursor-not-allowed disabled:opacity-60 dark:border-indigo-800 dark:bg-base-100 dark:text-indigo-300 dark:hover:bg-indigo-950/50"
            onClick={() => void loadConnections()}
            disabled={isLoading || isLoggingIn || isImporting}
            title={t('accounts.codex_refresh', '刷新 Codex 账号')}
          >
            <RefreshCw className={`h-3.5 w-3.5 ${isLoading ? 'animate-spin' : ''}`} />
            <span className="hidden sm:inline">{t('common.refresh', '刷新')}</span>
          </button>
          <button
            type="button"
            className="inline-flex min-h-9 items-center gap-1.5 rounded-lg bg-indigo-600 px-3 text-xs font-medium text-white shadow-sm transition-colors hover:bg-indigo-700 disabled:cursor-not-allowed disabled:opacity-60 dark:bg-indigo-500 dark:hover:bg-indigo-400"
            onClick={() => void handleLogin()}
            disabled={isLoggingIn || isImporting}
          >
            {isLoggingIn ? <Loader2 className="h-3.5 w-3.5 animate-spin" /> : <KeyRound className="h-3.5 w-3.5" />}
            {isLoggingIn ? t('accounts.codex_login_running', '等待登录...') : t('accounts.codex_login_button', 'Codex OAuth 登录')}
          </button>
        </div>
      </div>

      <div className="flex flex-wrap gap-2 border-b border-indigo-100/70 px-4 py-3 dark:border-indigo-900/40">
        <span className="rounded-lg border border-indigo-200 bg-indigo-100 px-2.5 py-1 text-xs font-bold text-indigo-900 shadow-sm dark:border-indigo-700 dark:bg-indigo-950/70 dark:text-indigo-100">
          {t('accounts.codex_total', '认证文件')} {connections.length}
        </span>
        <span className="rounded-lg bg-emerald-50 px-2.5 py-1 text-xs font-semibold text-emerald-800 dark:bg-emerald-950/50 dark:text-emerald-200">
          {t('accounts.codex_enabled_count', '已启用')} {enabledCount}
        </span>
        <span className="rounded-lg bg-slate-100 px-2.5 py-1 text-xs font-medium text-slate-600 dark:bg-slate-800 dark:text-slate-300">
          {t('accounts.codex_storage_hint', '凭证保存在本机私有 auth-files 目录')}
        </span>
      </div>

      {loadError && (
        <div className="border-b border-red-100 bg-red-50 px-4 py-2 text-xs text-red-700 dark:border-red-900/40 dark:bg-red-950/30 dark:text-red-300">
          {t('accounts.codex_load_failed', 'Codex 账号加载失败')}: {loadError}
        </div>
      )}

      <div className="grid gap-3 p-3 xl:grid-cols-2">
        {connections.length === 0 ? (
          <div className="flex min-h-14 items-center gap-2 rounded-xl border border-dashed border-indigo-300 bg-indigo-50/70 px-3 text-xs font-medium text-slate-700 xl:col-span-2 dark:border-indigo-700 dark:bg-indigo-950/30 dark:text-slate-200">
            <KeyRound className="h-4 w-4 text-indigo-600 dark:text-indigo-300" />
            {t('accounts.codex_empty', '尚未登录 Codex 账号')}
          </div>
        ) : (
          connections.map((connection) => {
            const credentialId = connection.credential.id;
            const identity = connection.identity.email || connection.identity.display_name || connection.credential.fingerprint;
            const lifecycle = connection.connection.enabled
              ? normalizeLifecycle(connection.credential.lifecycle)
              : 'disabled';
            const action = actionByCredential[credentialId];
            const plan = connectionPlan(connection);
            const models = modelsByCredential[credentialId];
            const quota = quotaByCredential[credentialId];
            const quotaError = quotaErrorsByCredential[credentialId];
            const quotaWindowItems = quotaWindows(quota?.rate_limit);
            const resetCreditCount = quota?.reset_credits_applicable_available_count
              ?? quota?.reset_credits_available_count
              ?? quota?.reset_credits?.length;
            const lifecycleLabel = {
              ready: t('accounts.codex_status_ready', '已连接'),
              pending: t('accounts.codex_status_pending', '验证中'),
              expired: t('accounts.codex_status_expired', '已过期'),
              revoked: t('accounts.codex_status_revoked', '已撤销'),
              unavailable: t('accounts.codex_status_unavailable', '不可用'),
              disabled: t('accounts.codex_status_disabled', '已停用'),
            }[lifecycle];
            return (
              <article className="overflow-hidden rounded-xl border border-slate-200 bg-white shadow-sm dark:border-slate-700 dark:bg-base-100" key={credentialId}>
                <div className="p-4">
                  <div className="flex items-start justify-between gap-3">
                    <div className="flex min-w-0 items-start gap-2.5">
                      <div className="mt-0.5 flex h-8 w-8 shrink-0 items-center justify-center rounded-lg bg-indigo-50 text-indigo-600 dark:bg-indigo-950/60 dark:text-indigo-300">
                        <FileKey2 className="h-4 w-4" />
                      </div>
                      <div className="min-w-0">
                        <h3 className="truncate text-sm font-semibold text-slate-950 dark:text-white">{identity}</h3>
                        <p className="mt-0.5 truncate text-[11px] font-medium text-slate-500 dark:text-slate-400">{connectionFileName(connection)}</p>
                      </div>
                    </div>
                    <span className={`inline-flex shrink-0 items-center gap-1 rounded-full border px-2 py-0.5 text-[11px] font-semibold ${lifecycleClasses[lifecycle]}`}>
                      {lifecycle === 'ready' ? <CheckCircle2 className="h-3 w-3" /> : lifecycle === 'pending' ? <Clock3 className="h-3 w-3" /> : <CircleAlert className="h-3 w-3" />}
                      {lifecycleLabel}
                    </span>
                  </div>

                  <div className="mt-3 grid grid-cols-2 gap-2 text-xs">
                    <div className="rounded-lg bg-slate-50 px-2.5 py-2 dark:bg-slate-800/80">
                      <div className="text-[10px] font-medium uppercase tracking-wide text-slate-500 dark:text-slate-400">{t('accounts.codex_plan', '计划')}</div>
                      <div className="mt-0.5 truncate font-semibold text-slate-800 dark:text-slate-100">{plan || '—'}</div>
                    </div>
                    <div className="rounded-lg bg-slate-50 px-2.5 py-2 dark:bg-slate-800/80">
                      <div className="text-[10px] font-medium uppercase tracking-wide text-slate-500 dark:text-slate-400">{t('accounts.codex_expires', '有效期至')}</div>
                      <div className="mt-0.5 truncate font-semibold text-slate-800 dark:text-slate-100">{formatExpiry(connection.credential.expires_at)}</div>
                    </div>
                  </div>

                  <div className="mt-3 flex flex-wrap items-center justify-between gap-2">
                    <label className="inline-flex cursor-pointer items-center gap-2 text-xs font-semibold text-slate-700 dark:text-slate-200">
                      <input
                        type="checkbox"
                        checked={connection.connection.enabled}
                        onChange={() => handleToggle(connection)}
                        disabled={action !== undefined}
                        className="h-4 w-4 rounded border-slate-300 text-indigo-600 focus:ring-indigo-500 dark:border-slate-600 dark:bg-slate-800"
                      />
                      <Power className="h-3.5 w-3.5" />
                      {connection.connection.enabled ? t('common.enabled', '已启用') : t('common.disabled', '已停用')}
                    </label>
                    <span className="truncate text-[11px] text-slate-500 dark:text-slate-400" title={connection.credential.fingerprint}>
                      {connection.credential.fingerprint}
                    </span>
                  </div>

                  <div className="mt-3 flex flex-wrap gap-1.5 border-t border-slate-100 pt-3 dark:border-slate-800">
                    <button type="button" className="inline-flex min-h-8 items-center gap-1 rounded-md border border-sky-200 px-2.5 text-[11px] font-semibold text-sky-700 hover:bg-sky-50 disabled:cursor-not-allowed disabled:opacity-60 dark:border-sky-800 dark:text-sky-300 dark:hover:bg-sky-950/50" onClick={() => void handleQuota(connection)} disabled={action !== undefined}>
                      {action === 'quota' ? <Loader2 className="h-3 w-3 animate-spin" /> : <Gauge className="h-3 w-3" />}
                      {t('accounts.codex_quota', '配额')}
                    </button>
                    <button type="button" className="inline-flex min-h-8 items-center gap-1 rounded-md border border-indigo-200 px-2.5 text-[11px] font-semibold text-indigo-700 hover:bg-indigo-50 disabled:cursor-not-allowed disabled:opacity-60 dark:border-indigo-800 dark:text-indigo-300 dark:hover:bg-indigo-950/50" onClick={() => void handleModels(connection)} disabled={action !== undefined}>
                      {action === 'models' ? <Loader2 className="h-3 w-3 animate-spin" /> : <List className="h-3 w-3" />}
                      {t('accounts.codex_models', '模型')}{models ? ` (${models.length})` : ''}
                    </button>
                    <button type="button" className="inline-flex min-h-8 items-center gap-1 rounded-md border border-slate-200 px-2.5 text-[11px] font-semibold text-slate-700 hover:bg-slate-50 disabled:cursor-not-allowed disabled:opacity-60 dark:border-slate-700 dark:text-slate-200 dark:hover:bg-slate-800" onClick={() => handleRefresh(connection)} disabled={action !== undefined}>
                      {action === 'refresh' ? <Loader2 className="h-3 w-3 animate-spin" /> : <RefreshCw className="h-3 w-3" />}
                      {t('accounts.codex_refresh_oauth', '刷新 OAuth')}
                    </button>
                    <button type="button" className="inline-flex min-h-8 items-center gap-1 rounded-md border border-slate-200 px-2.5 text-[11px] font-semibold text-slate-700 hover:bg-slate-50 disabled:cursor-not-allowed disabled:opacity-60 dark:border-slate-700 dark:text-slate-200 dark:hover:bg-slate-800" onClick={() => void handleExport(connection)} disabled={action !== undefined}>
                      {action === 'export' ? <Loader2 className="h-3 w-3 animate-spin" /> : <Download className="h-3 w-3" />}
                      {t('common.export', '导出')}
                    </button>
                    <button type="button" className="inline-flex min-h-8 items-center gap-1 rounded-md border border-rose-200 px-2.5 text-[11px] font-semibold text-rose-700 hover:bg-rose-50 disabled:cursor-not-allowed disabled:opacity-60 dark:border-rose-900/70 dark:text-rose-300 dark:hover:bg-rose-950/40" onClick={() => handleDelete(connection)} disabled={action !== undefined}>
                      {action === 'delete' ? <Loader2 className="h-3 w-3 animate-spin" /> : <Trash2 className="h-3 w-3" />}
                      {t('common.delete', '删除')}
                    </button>
                  </div>
                </div>

                <div className="border-t border-sky-100 bg-sky-50/70 px-4 py-3 dark:border-sky-900/50 dark:bg-sky-950/20">
                  <div className="flex items-center justify-between gap-2">
                    <div className="flex items-center gap-1.5 text-xs font-semibold text-slate-800 dark:text-slate-100">
                      <Gauge className="h-3.5 w-3.5 text-sky-600 dark:text-sky-300" />
                      {t('accounts.codex_quota_title', '剩余配额与重置')}
                      {quota?.plan_type ? <span className="rounded bg-white px-1.5 py-0.5 text-[10px] font-medium text-sky-700 dark:bg-slate-900 dark:text-sky-300">{quota.plan_type}</span> : null}
                    </div>
                    <button type="button" className="inline-flex items-center gap-1 text-[11px] font-semibold text-sky-700 hover:underline disabled:cursor-not-allowed disabled:opacity-60 dark:text-sky-300" onClick={() => void handleQuota(connection)} disabled={action !== undefined}>
                      {action === 'quota' ? <Loader2 className="h-3 w-3 animate-spin" /> : <RefreshCw className="h-3 w-3" />}
                      {t('accounts.codex_quota_refresh', '刷新配额')}
                    </button>
                  </div>

                  {quota ? (
                    <>
                      <div className="mt-2 grid gap-2 sm:grid-cols-2">
                        {quotaWindowItems.map((item) => (
                          <QuotaWindowCard key={item.label} label={item.label} window={item.window} resetLabel={t('accounts.codex_quota_reset', '重置')} remainingLabel={t('accounts.codex_quota_remaining', '剩余')} />
                        ))}
                        {quotaWindows(quota.code_review_rate_limit).map((item) => (
                          <QuotaWindowCard key={`code-review-${item.label}`} label={`Code review ${item.label}`} window={item.window} resetLabel={t('accounts.codex_quota_reset', '重置')} remainingLabel={t('accounts.codex_quota_remaining', '剩余')} />
                        ))}
                        {(quota.additional_rate_limits ?? []).flatMap((additional, index) => quotaWindows(additional.rate_limit).map((item) => (
                          <QuotaWindowCard
                            key={`additional-${index}-${item.label}`}
                            label={`${additional.limit_name || additional.metered_feature || t('accounts.codex_quota_additional', '附加')} ${item.label}`}
                            window={item.window}
                            resetLabel={t('accounts.codex_quota_reset', '重置')}
                            remainingLabel={t('accounts.codex_quota_remaining', '剩余')}
                          />
                        )))}
                      </div>
                      <div className="mt-2 flex flex-wrap gap-2 text-[11px] font-medium text-slate-700 dark:text-slate-200">
                        {resetCreditCount !== undefined ? (
                          <span className="inline-flex items-center gap-1 rounded-md border border-sky-200 bg-white px-2 py-1 dark:border-sky-800 dark:bg-slate-900/70">
                            <TimerReset className="h-3 w-3 text-sky-600 dark:text-sky-300" />
                            {t('accounts.codex_reset_credits', '可用重置次数')} {resetCreditCount}
                          </span>
                        ) : null}
                        {quota.subscription_expires_at ? (
                          <span className="rounded-md border border-slate-200 bg-white px-2 py-1 dark:border-slate-700 dark:bg-slate-900/70">
                            {t('accounts.codex_subscription_expires', '订阅有效期至')} {formatExpiry(quota.subscription_expires_at)}
                          </span>
                        ) : null}
                      </div>
                      {quota.reset_credits && quota.reset_credits.length > 0 ? (
                        <div className="mt-2 flex flex-wrap gap-1.5 text-[10px] font-medium text-slate-600 dark:text-slate-300">
                          {quota.reset_credits.slice(0, 4).map((credit, index) => (
                            <span className="rounded bg-white px-2 py-1 dark:bg-slate-900/70" key={credit.id || `credit-${index}`}>
                              {t('accounts.codex_reset_credit_expires', '重置额度到期')} {formatExpiry(credit.expires_at)}
                            </span>
                          ))}
                        </div>
                      ) : null}
                      {quota.reset_credits_error ? (
                        <div className="mt-2 text-[10px] font-medium text-amber-700 dark:text-amber-300">
                          {t('accounts.codex_reset_credits_unavailable', '重置额度明细暂不可用')}: {quota.reset_credits_error}
                        </div>
                      ) : null}
                    </>
                  ) : quotaError ? (
                    <div className="mt-2 rounded-lg border border-rose-200 bg-rose-50 px-3 py-2 text-[11px] font-medium text-rose-700 dark:border-rose-900/60 dark:bg-rose-950/30 dark:text-rose-300">
                      {t('accounts.codex_quota_failed', '配额获取失败')}: {quotaError}
                    </div>
                  ) : (
                    <div className="mt-2 text-[11px] font-medium text-slate-600 dark:text-slate-300">
                      {t('accounts.codex_quota_loading', '正在获取剩余配额和重置信息...')}
                    </div>
                  )}
                </div>

                {expandedModels === credentialId && models && (
                  <div className="border-t border-indigo-100 bg-indigo-50/60 px-4 py-3 dark:border-indigo-900/50 dark:bg-indigo-950/20">
                    <div className="mb-2 flex items-center justify-between gap-2">
                      <span className="text-xs font-semibold text-slate-800 dark:text-slate-100">{t('accounts.codex_supported_models', '支持的模型')}</span>
                      <button type="button" className="text-[11px] font-medium text-indigo-700 hover:underline dark:text-indigo-300" onClick={() => setExpandedModels(null)}>{t('common.close', '关闭')}</button>
                    </div>
                    <div className="max-h-48 overflow-auto rounded-lg border border-indigo-100 bg-white dark:border-indigo-900/60 dark:bg-slate-900/70">
                      {models.map((model) => (
                        <div className="grid grid-cols-[minmax(0,1fr)_minmax(0,0.8fr)] gap-3 border-b border-slate-100 px-3 py-2 text-[11px] last:border-b-0 dark:border-slate-800" key={model.id}>
                          <span className="truncate font-semibold text-slate-800 dark:text-slate-100" title={model.id}>{model.id}</span>
                          <span className="truncate text-slate-500 dark:text-slate-400" title={model.display_name || model.id}>{model.display_name || model.id}</span>
                        </div>
                      ))}
                    </div>
                  </div>
                )}

                <details className="border-t border-slate-100 px-4 py-2.5 text-[11px] dark:border-slate-800">
                  <summary className="cursor-pointer font-semibold text-slate-600 hover:text-indigo-700 dark:text-slate-300 dark:hover:text-indigo-300">{t('accounts.codex_details', '认证文件详情')}</summary>
                  <dl className="mt-2 grid gap-1.5 text-slate-600 dark:text-slate-300">
                    <div className="flex justify-between gap-3"><dt>{t('accounts.codex_provider', '供应商类型')}</dt><dd className="font-medium text-slate-800 dark:text-slate-100">{connection.connection.provider_kind}</dd></div>
                    <div className="flex justify-between gap-3"><dt>{t('accounts.codex_endpoint', '上游端点')}</dt><dd className="max-w-[70%] truncate font-medium text-slate-800 dark:text-slate-100" title={connection.connection.endpoint}>{connection.connection.endpoint}</dd></div>
                    <div className="flex justify-between gap-3"><dt>{t('accounts.codex_status', '连接状态')}</dt><dd className="font-medium text-slate-800 dark:text-slate-100">{connection.connection.status}</dd></div>
                  </dl>
                </details>
              </article>
            );
          })
        )}
      </div>
    </section>
  );
}
