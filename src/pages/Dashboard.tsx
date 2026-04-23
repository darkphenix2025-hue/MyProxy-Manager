import { RefreshCw, Zap, TrendingUp, Cpu, Users } from 'lucide-react';
import { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { AreaChart, Area, BarChart, Bar, XAxis, YAxis, CartesianGrid, Tooltip, ResponsiveContainer, PieChart, Pie, Cell, Legend } from 'recharts';
import { useAccountStore } from '../stores/useAccountStore';
import { request as invoke } from '../utils/request';

// --- Token Stats types ---
interface TokenStatsAggregated {
    period: string;
    total_input_tokens: number;
    total_output_tokens: number;
    total_tokens: number;
    request_count: number;
}
interface AccountTokenStats {
    account_email: string;
    total_input_tokens: number;
    total_output_tokens: number;
    total_tokens: number;
    request_count: number;
}
interface ModelTokenStats {
    model: string;
    total_input_tokens: number;
    total_output_tokens: number;
    total_tokens: number;
    request_count: number;
}
interface ModelTrendPoint {
    period: string;
    model_data: Record<string, number>;
}
interface AccountTrendPoint {
    period: string;
    account_data: Record<string, number>;
}
interface TokenStatsSummary {
    total_input_tokens: number;
    total_output_tokens: number;
    total_tokens: number;
    total_requests: number;
    unique_accounts: number;
}
type TimeRange = 'hourly' | 'daily' | 'weekly';

const MODEL_COLORS = [
    '#3b82f6', '#8b5cf6', '#ec4899', '#f59e0b', '#10b981',
    '#06b6d4', '#6366f1', '#f43f5e', '#84cc16', '#a855f7',
    '#14b8a6', '#f97316', '#64748b', '#0ea5e9', '#d946ef',
];
const COLORS = ['#3b82f6', '#8b5cf6', '#ec4899', '#f59e0b', '#10b981', '#06b6d4', '#6366f1', '#f43f5e'];

const formatNumber = (num: number): string => {
    if (num >= 1000000) return `${(num / 1000000).toFixed(1)}M`;
    if (num >= 1000) return `${(num / 1000).toFixed(1)}K`;
    return num.toString();
};
const shortenModelName = (model: string): string =>
    model.replace('gemini-', 'g-').replace('claude-', 'c-').replace('-preview', '').replace('-latest', '');

// --- Dashboard ---
function Dashboard() {
    const { t } = useTranslation();
    const { fetchAccounts } = useAccountStore();

    useEffect(() => {
        fetchAccounts();
    }, []);

    // Token Stats state
    const [timeRange, setTimeRange] = useState<TimeRange>('daily');
    const [summary, setSummary] = useState<TokenStatsSummary | null>(null);
    const [chartData, setChartData] = useState<TokenStatsAggregated[]>([]);
    const [modelTrendData, setModelTrendData] = useState<any[]>([]);
    const [allModels, setAllModels] = useState<string[]>([]);
    const [accountTrendData, setAccountTrendData] = useState<any[]>([]);
    const [allAccounts, setAllAccounts] = useState<string[]>([]);
    const [modelData, setModelData] = useState<ModelTokenStats[]>([]);
    const [accountData, setAccountData] = useState<AccountTokenStats[]>([]);
    const [loadingStats, setLoadingStats] = useState(true);
    const [viewMode, setViewMode] = useState<'model' | 'account'>('model');

    const fetchStats = async () => {
        setLoadingStats(true);
        try {
            let hours = 24;
            switch (timeRange) {
                case 'hourly': hours = 24; break;
                case 'daily': hours = 168; break;
                case 'weekly': hours = 720; break;
            }

            const [data, modelTrend, accountTrend, accountsStats, modelsStats, summaryData] = await Promise.all([
                invoke<TokenStatsAggregated[]>(timeRange === 'weekly'
                    ? 'get_token_stats_weekly' : timeRange === 'daily'
                        ? 'get_token_stats_daily' : 'get_token_stats_hourly',
                    timeRange === 'weekly' ? { weeks: 4 } : timeRange === 'daily' ? { days: 7 } : { hours: 24 }),
                invoke<ModelTrendPoint[]>(timeRange === 'weekly' ? 'get_token_stats_model_trend_daily' : 'get_token_stats_model_trend_hourly',
                    timeRange === 'weekly' ? { days: 30 } : { hours: 24 }),
                invoke<AccountTrendPoint[]>(timeRange === 'weekly' ? 'get_token_stats_account_trend_daily' : 'get_token_stats_account_trend_hourly',
                    timeRange === 'weekly' ? { days: 30 } : { hours: 24 }),
                invoke<AccountTokenStats[]>('get_token_stats_by_account', { hours }),
                invoke<ModelTokenStats[]>('get_token_stats_by_model', { hours }),
                invoke<TokenStatsSummary>('get_token_stats_summary', { hours }),
            ]);

            setChartData(data);
            setSummary(summaryData);
            setAccountData(accountsStats);
            setModelData(modelsStats);

            const models = new Set<string>();
            modelTrend.forEach(p => Object.keys(p.model_data).forEach(m => models.add(m)));
            const modelList = Array.from(models);
            setAllModels(modelList);
            setModelTrendData(modelTrend.map(p => {
                const row: Record<string, any> = { period: p.period };
                modelList.forEach(m => { row[m] = p.model_data[m] || 0; });
                return row;
            }));

            const accountsSet = new Set<string>();
            accountTrend.forEach(p => Object.keys(p.account_data).forEach(a => accountsSet.add(a)));
            const accountList = Array.from(accountsSet);
            setAllAccounts(accountList);
            setAccountTrendData(accountTrend.map(p => {
                const row: Record<string, any> = { period: p.period };
                accountList.forEach(a => { row[a] = p.account_data[a] || 0; });
                return row;
            }));
        } catch (error) {
            console.error('Failed to fetch token stats:', error);
        } finally {
            setLoadingStats(false);
        }
    };

    useEffect(() => { fetchStats(); }, [timeRange]);

    const pieData = accountData.slice(0, 8).map((a, i) => ({
        name: a.account_email.split('@')[0] + '...',
        value: a.total_tokens,
        fullEmail: a.account_email,
        color: COLORS[i % COLORS.length],
    }));

    // Custom tooltip for area chart
    const CustomTrendTooltip = ({ active, payload, label }: any) => {
        if (!active || !payload?.length) return null;
        const period = timeRange === 'hourly' ? (label || '').split(' ')[1] : label;
        return (
            <div className="bg-white/95 dark:bg-gray-800/95 backdrop-blur-sm p-3 rounded-xl shadow-xl border border-gray-100 dark:border-gray-700 text-xs z-[100] pointer-events-none">
                <p className="font-semibold text-gray-700 dark:text-gray-200 mb-2">{period}</p>
                <div className="space-y-1">
                    {payload.map((entry: any, idx: number) => (
                        <div key={idx} className="flex items-center gap-2">
                            <div className="w-2 h-2 rounded-full flex-shrink-0" style={{ backgroundColor: entry.color }} />
                            <span className="text-gray-500 dark:text-gray-400 truncate max-w-[120px]" title={viewMode === 'model' ? entry.dataKey : entry.dataKey}>{viewMode === 'model' ? shortenModelName(entry.dataKey) : entry.dataKey}:</span>
                            <span className="font-mono font-medium text-gray-700 dark:text-gray-200">{formatNumber(entry.value)}</span>
                        </div>
                    ))}
                </div>
            </div>
        );
    };

    const SimpleTooltip = ({ active, payload }: any) => {
        if (!active || !payload?.length) return null;
        return (
            <div className="bg-white/95 dark:bg-gray-800/95 backdrop-blur-sm p-2.5 rounded-xl shadow-xl border border-gray-100 dark:border-gray-700 text-xs z-[100] pointer-events-none">
                <div className="space-y-1">
                    {payload.map((e: any, i: number) => (
                        <div key={i} className="flex items-center gap-2">
                            <div className="w-2 h-2 rounded-full" style={{ backgroundColor: e.color }} />
                            <span className="text-gray-500 dark:text-gray-400">{e.name}:</span>
                            <span className="font-mono font-medium text-gray-700 dark:text-gray-200">{formatNumber(e.value)}</span>
                        </div>
                    ))}
                </div>
            </div>
        );
    };

    const CustomPieTooltip = ({ active, payload }: any) => {
        if (!active || !payload?.length) return null;
        const entry = payload[0];
        return (
            <div className="bg-white/95 dark:bg-gray-800/95 backdrop-blur-sm p-2.5 rounded-xl shadow-xl border border-gray-100 dark:border-gray-700 text-xs z-[100] pointer-events-none">
                <div className="flex items-center gap-2">
                    <div className="w-2 h-2 rounded-full" style={{ backgroundColor: entry.payload.color || entry.color }} />
                    <span className="text-gray-500 dark:text-gray-400">{entry.payload.fullEmail || entry.name}:</span>
                    <span className="font-mono font-medium text-gray-700 dark:text-gray-200">{formatNumber(entry.value)}</span>
                </div>
            </div>
        );
    };

    return (
        <div className="h-full w-full overflow-y-auto">
            <div className="p-5 space-y-4 max-w-7xl mx-auto">
                {/* TokenStats 工具栏 */}
                <div className="flex justify-end items-center gap-2">
                    <div className="flex bg-gray-100 dark:bg-gray-800 rounded-lg p-1">
                        {(['hourly', 'daily', 'weekly'] as TimeRange[]).map(range => (
                            <button key={range} onClick={() => setTimeRange(range)}
                                className={`px-3 py-1.5 rounded-md text-sm font-medium transition-colors flex items-center gap-1.5 ${timeRange === range ? 'bg-white dark:bg-gray-700 text-blue-600 shadow-sm' : 'text-gray-600 dark:text-gray-400 hover:text-gray-800'}`}
                            >
                                {t(`token_stats.${range}`, range === 'hourly' ? '小时' : range === 'daily' ? '日' : '周')}
                            </button>
                        ))}
                    </div>
                    <button onClick={fetchStats} disabled={loadingStats} className="p-2 rounded-lg bg-blue-500 text-white hover:bg-blue-600 transition-colors disabled:opacity-50">
                        <RefreshCw className={`w-4 h-4 ${loadingStats ? 'animate-spin' : ''}`} />
                    </button>
                </div>

                {/* Token Stats 摘要卡片 */}
                {summary && (
                    <div className="grid grid-cols-2 md:grid-cols-5 gap-3">
                        <div className="bg-white dark:bg-base-100 rounded-xl p-4 shadow-sm border border-gray-100 dark:border-base-200">
                            <div className="flex items-center gap-2 text-gray-500 dark:text-gray-400 text-xs mb-1">
                                <Zap className="w-4 h-4 text-blue-500" />{t('token_stats.total_tokens', '总 Token')}
                            </div>
                            <div className="text-2xl font-bold text-gray-800 dark:text-white">{formatNumber(summary.total_tokens)}</div>
                        </div>
                        <div className="bg-white dark:bg-base-100 rounded-xl p-4 shadow-sm border border-gray-100 dark:border-base-200">
                            <div className="flex items-center gap-2 text-blue-600/80 dark:text-blue-400/80 text-xs mb-1">
                                <TrendingUp className="w-4 h-4 text-blue-500" />{t('token_stats.input_tokens', '输入 Token')}
                            </div>
                            <div className="text-2xl font-bold text-blue-600 dark:text-blue-400">{formatNumber(summary.total_input_tokens)}</div>
                        </div>
                        <div className="bg-white dark:bg-base-100 rounded-xl p-4 shadow-sm border border-gray-100 dark:border-base-200">
                            <div className="flex items-center gap-2 text-purple-600/80 dark:text-purple-400/80 text-xs mb-1">
                                <TrendingUp className="w-4 h-4 rotate-180 text-purple-500" />{t('token_stats.output_tokens', '输出 Token')}
                            </div>
                            <div className="text-2xl font-bold text-purple-600 dark:text-purple-400">{formatNumber(summary.total_output_tokens)}</div>
                        </div>
                        <div className="bg-white dark:bg-base-100 rounded-xl p-4 shadow-sm border border-gray-100 dark:border-base-200">
                            <div className="flex items-center gap-2 text-green-600/80 dark:text-green-400/80 text-xs mb-1">
                                <Users className="w-4 h-4 text-green-500" />{t('token_stats.accounts_used', '活跃账号')}
                            </div>
                            <div className="text-2xl font-bold text-green-600 dark:text-green-400">{summary.unique_accounts}</div>
                        </div>
                        <div className="bg-white dark:bg-base-100 rounded-xl p-4 shadow-sm border border-gray-100 dark:border-base-200">
                            <div className="flex items-center gap-2 text-orange-600/80 dark:text-orange-400/80 text-xs mb-1">
                                <Cpu className="w-4 h-4 text-orange-500" />{t('token_stats.models_used', '使用模型')}
                            </div>
                            <div className="text-2xl font-bold text-orange-600 dark:text-orange-400">{modelData.length}</div>
                        </div>
                    </div>
                )}

                {/* 分模型/分账号趋势 */}
                <div className="bg-white dark:bg-base-100 rounded-xl p-4 shadow-sm border border-gray-100 dark:border-base-200">
                    <div className="flex items-center justify-between mb-4">
                        <h2 className="text-base font-semibold text-gray-800 dark:text-white flex items-center gap-2">
                            {viewMode === 'model' ? <Cpu className="w-5 h-5 text-purple-500" /> : <Users className="w-5 h-5 text-green-500" />}
                            {viewMode === 'model' ? t('token_stats.model_trend', '分模型使用趋势') : t('token_stats.account_trend', '分账号使用趋势')}
                        </h2>
                        <div className="flex bg-gray-100/80 dark:bg-gray-700/50 rounded-lg p-1">
                            <button onClick={() => setViewMode('model')}
                                className={`px-3 py-1 text-xs font-medium rounded-md transition-all ${viewMode === 'model' ? 'bg-white dark:bg-gray-600 text-blue-600 dark:text-blue-400 shadow-sm' : 'text-gray-500 dark:text-gray-400'}`}>
                                {t('token_stats.by_model', '按模型')}
                            </button>
                            <button onClick={() => setViewMode('account')}
                                className={`px-3 py-1 text-xs font-medium rounded-md transition-all ${viewMode === 'account' ? 'bg-white dark:bg-gray-600 text-blue-600 dark:text-blue-400 shadow-sm' : 'text-gray-500 dark:text-gray-400'}`}>
                                {t('token_stats.by_account_view', '按账号')}
                            </button>
                        </div>
                    </div>
                    <div className="h-64">
                        {(modelTrendData.length > 0 && allModels.length > 0) || (accountTrendData.length > 0 && allAccounts.length > 0) ? (
                            <ResponsiveContainer width="100%" height="100%">
                                <AreaChart data={viewMode === 'model' ? modelTrendData : accountTrendData}>
                                    <CartesianGrid strokeDasharray="3 3" vertical={false} stroke="#374151" strokeOpacity={0.15} />
                                    <XAxis dataKey="period" tick={{ fontSize: 11, fill: '#6b7280' }}
                                        tickFormatter={(val) => timeRange === 'hourly' ? (val.split(' ')[1] || val) : timeRange === 'daily' ? val.split('-').slice(1).join('/') : val}
                                        axisLine={false} tickLine={false} dy={10} />
                                    <YAxis tick={{ fontSize: 11, fill: '#6b7280' }} tickFormatter={(val) => formatNumber(val)} axisLine={false} tickLine={false} />
                                    <Tooltip content={<CustomTrendTooltip />} cursor={{ stroke: '#6b7280', strokeWidth: 1, strokeDasharray: '4 4', fill: 'transparent' }} allowEscapeViewBox={{ x: true, y: true }} />
                                    <Legend formatter={(value) => viewMode === 'model' ? shortenModelName(value) : value.split('@')[0]}
                                        wrapperStyle={{ fontSize: '11px', paddingTop: '10px', maxHeight: '60px', overflowY: 'auto' }} />
                                    {(viewMode === 'model' ? allModels : allAccounts).map((item, idx) => (
                                        <Area key={item} type="monotone" dataKey={item} stackId="1"
                                            stroke={viewMode === 'model' ? MODEL_COLORS[idx % MODEL_COLORS.length] : COLORS[idx % COLORS.length]}
                                            fill={viewMode === 'model' ? MODEL_COLORS[idx % MODEL_COLORS.length] : COLORS[idx % COLORS.length]}
                                            fillOpacity={0.6} />
                                    ))}
                                </AreaChart>
                            </ResponsiveContainer>
                        ) : (
                            <div className="h-full flex items-center justify-center text-gray-400">
                                {loadingStats ? t('common.loading', '加载中...') : t('token_stats.no_data', '暂无数据')}
                            </div>
                        )}
                    </div>
                </div>

                {/* 柱状图 + 饼图 */}
                <div className="grid grid-cols-1 lg:grid-cols-3 gap-4">
                    <div className="lg:col-span-2 bg-white dark:bg-base-100 rounded-xl p-4 shadow-sm border border-gray-100 dark:border-base-200">
                        <h2 className="text-base font-semibold text-gray-800 dark:text-white mb-3">{t('token_stats.usage_trend', 'Token 使用趋势')}</h2>
                        <div className="h-48">
                            {chartData.length > 0 ? (
                                <ResponsiveContainer width="100%" height="100%">
                                    <BarChart data={chartData}>
                                        <CartesianGrid strokeDasharray="3 3" vertical={false} stroke="#374151" strokeOpacity={0.15} />
                                        <XAxis dataKey="period" tick={{ fontSize: 11, fill: '#6b7280' }}
                                            tickFormatter={(val) => timeRange === 'hourly' ? (val.split(' ')[1] || val) : timeRange === 'daily' ? val.split('-').slice(1).join('/') : val}
                                            axisLine={false} tickLine={false} dy={10} />
                                        <YAxis tick={{ fontSize: 11, fill: '#6b7280' }} tickFormatter={(val) => formatNumber(val)} axisLine={false} tickLine={false} />
                                        <Tooltip content={<SimpleTooltip />} cursor={{ fill: 'transparent' }} allowEscapeViewBox={{ x: true, y: true }} />
                                        <Bar dataKey="total_input_tokens" name="Input" fill="#3b82f6" radius={[4, 4, 0, 0]} maxBarSize={50} />
                                        <Bar dataKey="total_output_tokens" name="Output" fill="#8b5cf6" radius={[4, 4, 0, 0]} maxBarSize={50} />
                                    </BarChart>
                                </ResponsiveContainer>
                            ) : (
                                <div className="h-full flex items-center justify-center text-gray-400">
                                    {loadingStats ? t('common.loading', '加载中...') : t('token_stats.no_data', '暂无数据')}
                                </div>
                            )}
                        </div>
                    </div>
                    <div className="bg-white dark:bg-base-100 rounded-xl p-4 shadow-sm border border-gray-100 dark:border-base-200">
                        <h2 className="text-base font-semibold text-gray-800 dark:text-white mb-3">{t('token_stats.by_account', '分账号统计')}</h2>
                        <div className="h-40">
                            {pieData.length > 0 ? (
                                <ResponsiveContainer width="100%" height="100%">
                                    <PieChart>
                                        <Pie data={pieData} cx="50%" cy="50%" innerRadius={35} outerRadius={60} paddingAngle={2} dataKey="value">
                                            {pieData.map((entry, idx) => <Cell key={idx} fill={entry.color} />)}
                                        </Pie>
                                        <Tooltip content={<CustomPieTooltip />} allowEscapeViewBox={{ x: true, y: true }} />
                                    </PieChart>
                                </ResponsiveContainer>
                            ) : (
                                <div className="h-full flex items-center justify-center text-gray-400">
                                    {loadingStats ? t('common.loading', '加载中...') : t('token_stats.no_data', '暂无数据')}
                                </div>
                            )}
                        </div>
                        <div className="mt-3 space-y-1.5 max-h-28 overflow-y-auto">
                            {accountData.slice(0, 5).map((acc, idx) => (
                                <div key={acc.account_email} className="flex items-center justify-between text-xs">
                                    <div className="flex items-center gap-1.5">
                                        <div className="w-2.5 h-2.5 rounded-full flex-shrink-0" style={{ backgroundColor: COLORS[idx % COLORS.length] }} />
                                        <span className="text-gray-600 dark:text-gray-300 truncate max-w-[100px]">{acc.account_email.split('@')[0]}</span>
                                    </div>
                                    <span className="font-medium text-gray-800 dark:text-white">{formatNumber(acc.total_tokens)}</span>
                                </div>
                            ))}
                        </div>
                    </div>
                </div>

                {/* 分模型详细统计表 */}
                {modelData.length > 0 && (
                    <div className="bg-white dark:bg-base-100 rounded-xl p-4 shadow-sm border border-gray-100 dark:border-base-200 overflow-x-auto">
                        <h2 className="text-base font-semibold text-gray-800 dark:text-white mb-3 flex items-center gap-2">
                            <Cpu className="w-5 h-5 text-blue-500" />{t('token_stats.model_details', '分模型详细统计')}
                        </h2>
                        <table className="w-full text-sm min-w-[600px]">
                            <thead>
                                <tr className="border-b border-gray-200 dark:border-gray-700">
                                    <th className="text-left py-2 px-3 font-medium text-gray-500 dark:text-gray-400 text-xs">{t('token_stats.model', '模型')}</th>
                                    <th className="text-right py-2 px-3 font-medium text-gray-500 dark:text-gray-400 text-xs">{t('token_stats.requests', '请求数')}</th>
                                    <th className="text-right py-2 px-3 font-medium text-gray-500 dark:text-gray-400 text-xs">{t('token_stats.input', '输入')}</th>
                                    <th className="text-right py-2 px-3 font-medium text-gray-500 dark:text-gray-400 text-xs">{t('token_stats.output', '输出')}</th>
                                    <th className="text-right py-2 px-3 font-medium text-gray-500 dark:text-gray-400 text-xs">{t('token_stats.total', '合计')}</th>
                                    <th className="text-right py-2 px-3 font-medium text-gray-500 dark:text-gray-400 text-xs">{t('token_stats.percentage', '占比')}</th>
                                </tr>
                            </thead>
                            <tbody>
                                {modelData.map((model, idx) => {
                                    const pct = summary ? ((model.total_tokens / summary.total_tokens) * 100).toFixed(1) : '0';
                                    return (
                                        <tr key={model.model} className="border-b border-gray-100 dark:border-gray-700/50 hover:bg-gray-50 dark:hover:bg-gray-700/30">
                                            <td className="py-2 px-3">
                                                <div className="flex items-center gap-2">
                                                    <div className="w-2.5 h-2.5 rounded-full" style={{ backgroundColor: MODEL_COLORS[idx % MODEL_COLORS.length] }} />
                                                    <span className="text-gray-800 dark:text-white text-xs font-medium">{model.model}</span>
                                                </div>
                                            </td>
                                            <td className="py-2 px-3 text-right text-gray-600 dark:text-gray-300 text-xs">{model.request_count.toLocaleString()}</td>
                                            <td className="py-2 px-3 text-right text-blue-600 text-xs">{formatNumber(model.total_input_tokens)}</td>
                                            <td className="py-2 px-3 text-right text-purple-600 text-xs">{formatNumber(model.total_output_tokens)}</td>
                                            <td className="py-2 px-3 text-right font-semibold text-gray-800 dark:text-white text-xs">{formatNumber(model.total_tokens)}</td>
                                            <td className="py-2 px-3 text-right">
                                                <div className="flex items-center justify-end gap-2">
                                                    <div className="w-16 bg-gray-200 dark:bg-gray-700 rounded-full h-1.5">
                                                        <div className="h-1.5 rounded-full" style={{ width: `${pct}%`, backgroundColor: MODEL_COLORS[idx % MODEL_COLORS.length] }} />
                                                    </div>
                                                    <span className="text-gray-600 dark:text-gray-300 text-xs w-10 text-right">{pct}%</span>
                                                </div>
                                            </td>
                                        </tr>
                                    );
                                })}
                            </tbody>
                        </table>
                    </div>
                )}
            </div>
        </div>
    );
}

export default Dashboard;
