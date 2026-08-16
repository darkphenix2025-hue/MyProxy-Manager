import { createBrowserRouter, RouterProvider } from 'react-router-dom';
import { lazy, Suspense, useEffect } from 'react';

const Layout = lazy(() => import('./components/layout/Layout'));
const Dashboard = lazy(() => import('./pages/Dashboard'));
const Accounts = lazy(() => import('./pages/Accounts'));
const Settings = lazy(() => import('./pages/Settings'));
const ApiProxy = lazy(() => import('./pages/ApiProxy'));
const Monitor = lazy(() => import('./pages/Monitor'));
const TokenStats = lazy(() => import('./pages/TokenStats'));
const Security = lazy(() => import('./pages/Security'));
const ThemeManager = lazy(() => import('./components/common/ThemeManager'));
const UserToken = lazy(() => import('./pages/UserToken'));
const ProviderManager = lazy(() => import('./pages/ProviderManager'));
const ServiceConfig = lazy(() => import('./pages/ServiceConfig'));
const RouteManager = lazy(() => import('./pages/RouteManager'));
const DebugConsole = lazy(() => import('./components/debug/DebugConsole'));
import { useConfigStore } from './stores/useConfigStore';
import { useAccountStore } from './stores/useAccountStore';
import { useTranslation } from 'react-i18next';
import { listen } from '@tauri-apps/api/event';
import { isTauri } from './utils/env';
const AdminAuthGuard = lazy(() => import('./components/common/AdminAuthGuard').then(module => ({ default: module.AdminAuthGuard })));
const UpdateManager = lazy(() => import('./components/update/UpdateManager'));
import { Agentation } from 'agentation';

const router = createBrowserRouter([
  {
    path: '/',
    element: <Layout />,
    children: [
      {
        index: true,
        element: <Dashboard />,
      },
      {
        path: 'accounts',
        element: <Accounts />,
      },
      {
        path: 'api-proxy',
        element: <ApiProxy />,
      },
      {
        path: 'service-config',
        element: <ServiceConfig />,
      },
      {
        path: 'route-manage',
        element: <RouteManager />,
      },
      {
        path: 'monitor',
        element: <Monitor />,
      },
      {
        path: 'token-stats',
        element: <TokenStats />,
      },
      {
        path: 'user-token',
        element: <UserToken />,
      },
      {
        path: 'security',
        element: <Security />,
      },
      {
        path: 'providers',
        element: <ProviderManager />,
      },
      {
        path: 'settings',
        element: <Settings />,
      },
    ],
  },
]);

function App() {
  const { config, loadConfig } = useConfigStore();
  const { fetchCurrentAccount, fetchAccounts } = useAccountStore();
  const { i18n } = useTranslation();

  useEffect(() => {
    loadConfig();
  }, [loadConfig]);

  // Sync language from config
  useEffect(() => {
    if (config?.language) {
      i18n.changeLanguage(config.language);
      // Support RTL
      if (config.language === 'ar') {
        document.documentElement.dir = 'rtl';
      } else {
        document.documentElement.dir = 'ltr';
      }
    }
  }, [config?.language, i18n]);

  // Listen for tray events
  useEffect(() => {
    if (!isTauri()) return;
    const unlistenPromises: Promise<() => void>[] = [];

    // 监听托盘切换账号事件
    unlistenPromises.push(
      listen('tray://account-switched', () => {
        fetchCurrentAccount();
        fetchAccounts();
      })
    );

    // 监听托盘刷新事件
    unlistenPromises.push(
      listen('tray://refresh-current', () => {
        fetchCurrentAccount();
        fetchAccounts();
      })
    );

    // 监听后端全量刷新事件 (Command / Scheduler)
    unlistenPromises.push(
      listen('accounts://refreshed', () => {
        fetchCurrentAccount();
        fetchAccounts();
      })
    );

    // Cleanup
    return () => {
      Promise.all(unlistenPromises).then(unlisteners => {
        unlisteners.forEach(unlisten => unlisten());
      });
    };
  }, [fetchCurrentAccount, fetchAccounts]);

  return (
    <Suspense fallback={<div aria-busy="true" />}>
      <AdminAuthGuard>
        <ThemeManager />
        <DebugConsole />
        <UpdateManager />
        <RouterProvider router={router} />
        {import.meta.env.DEV && <Agentation />}
      </AdminAuthGuard>
    </Suspense>
  );
}

export default App;
