import React, { useEffect, useState, useRef, useMemo } from 'react';
import { listen } from '@tauri-apps/api/event';
import ModalDialog from '../common/ModalDialog';
import { useTranslation } from 'react-i18next';
import { request as invoke } from '../../utils/request';
import { Trash2, Search, X, Copy, CheckCircle, ChevronLeft, ChevronRight, RefreshCw, User, ArrowRight, MessageSquare, Server, Cpu, AlertTriangle, ChevronDown, ChevronUp, Eye, Repeat2 } from 'lucide-react';

import { AppConfig } from '../../types/config';
import { formatCompactNumber } from '../../utils/format';
import { useAccountStore } from '../../stores/useAccountStore';
import { useLlmLogging } from '../../stores/useLlmLogging';
import { isTauri } from '../../utils/env';
import { copyToClipboard } from '../../utils/clipboard';
import LlmLogViewer from '../settings/LlmLogViewer';

// Module-level cache: persists expanded state even when components unmount/remount
const truncatableExpandedCache = new Map<string, boolean>();

/** [R2-UX] 可展开/折叠的截断文本组件 */
interface TruncatableTextProps {
    text: string;
    maxLength: number;
    className?: string;
    as?: 'pre' | 'p' | 'span' | 'div';
}

const TruncatableText: React.FC<TruncatableTextProps> = ({ text, maxLength, className = '', as = 'p' }) => {
    if (!text) return null;
    const isTruncated = text.length > maxLength;
    const cacheKey = text.length > 100 ? text.slice(0, 100) : text;
    const [expanded, setExpanded] = useState(() => truncatableExpandedCache.get(cacheKey) ?? false);
    const displayText = expanded || !isTruncated ? text : text.slice(0, maxLength) + '...';
    const Tag = as;

    return (
        <div>
            <Tag className={className}>{displayText}</Tag>
            {isTruncated && (
                <button
                    type="button"
                    className="mt-1 text-[9px] font-bold text-blue-500 dark:text-blue-400 hover:text-blue-700 dark:hover:text-blue-300 cursor-pointer select-none"
                    onClick={() => {
                        const newState = !expanded;
                        truncatableExpandedCache.set(cacheKey, newState);
                        setExpanded(newState);
                    }}
                >
                    {expanded ? '[收起]' : `[展开完整内容 (${text.length} 字符)]`}
                </button>
            )}
        </div>
    );
};

/** 行级 Diff 对比组件 — 同步滚动，高亮差异行 */
const DiffView: React.FC<{ left: string; right: string; leftLabel: string; rightLabel: string }> = ({ left, right, leftLabel, rightLabel }) => {
    const leftRef = useRef<HTMLDivElement>(null);
    const rightRef = useRef<HTMLDivElement>(null);
    const syncingRef = useRef(false);

    const onSyncScroll = (source: 'left' | 'right') => (e: React.UIEvent<HTMLDivElement>) => {
        if (syncingRef.current) return;
        syncingRef.current = true;
        const target = source === 'left' ? rightRef.current : leftRef.current;
        const sourceEl = e.currentTarget;
        if (target) {
            target.scrollTop = sourceEl.scrollTop;
            target.scrollLeft = sourceEl.scrollLeft;
        }
        requestAnimationFrame(() => { syncingRef.current = false; });
    };

    const leftJson = useMemo(() => { try { return JSON.parse(left || '{}'); } catch { return {}; } }, [left]);
    const rightJson = useMemo(() => { try { return JSON.parse(right || '{}'); } catch { return {}; } }, [right]);

    // Collect all keys from both sides, preferring left order (preserve original)
    const allKeys = useMemo(() => {
        const keySet = new Set<string>();
        const keys: string[] = [];
        if (typeof leftJson === 'object' && !Array.isArray(leftJson) && leftJson) {
            Object.keys(leftJson).forEach(k => { if (!keySet.has(k)) { keySet.add(k); keys.push(k); } });
        }
        if (typeof rightJson === 'object' && !Array.isArray(rightJson) && rightJson) {
            Object.keys(rightJson).forEach(k => { if (!keySet.has(k)) { keySet.add(k); keys.push(k); } });
        }
        return keys;
    }, [leftJson, rightJson]);

    // Deep equality check
    const deepEqual = (a: any, b: any): boolean => {
        if (a === b) return true;
        if (typeof a !== typeof b) return false;
        if (a === null || b === null) return a === b;
        if (Array.isArray(a)) {
            if (!Array.isArray(b) || a.length !== b.length) return false;
            return a.every((item, i) => deepEqual(item, b[i]));
        }
        if (typeof a === 'object') {
            const aKeys = Object.keys(a);
            const bKeys = Object.keys(b);
            if (aKeys.length !== bKeys.length) return false;
            return aKeys.every(k => k in b && deepEqual(a[k], b[k]));
        }
        return false;
    };

    // Build diff rows
    type DiffResult = { type: 'same' | 'changed' | 'left-only' | 'right-only' };
    type DiffRow = { key: string; leftVal: any; rightVal: any; result: DiffResult };

    const buildDiffRows = useMemo(() => {
        const rows: DiffRow[] = [];
        for (const key of allKeys) {
            const hasLeft = key in leftJson;
            const hasRight = key in rightJson;
            if (hasLeft && hasRight) {
                const lv = leftJson[key];
                const rv = rightJson[key];
                rows.push({
                    key,
                    leftVal: lv,
                    rightVal: rv,
                    result: deepEqual(lv, rv) ? { type: 'same' } : { type: 'changed' },
                });
            } else if (hasLeft) {
                rows.push({ key, leftVal: leftJson[key], rightVal: undefined, result: { type: 'left-only' } });
            } else {
                rows.push({ key, leftVal: undefined, rightVal: rightJson[key], result: { type: 'right-only' } });
            }
        }
        return rows;
    }, [allKeys, leftJson, rightJson]);

    const stats = useMemo(() => ({
        changed: buildDiffRows.filter(r => r.result.type === 'changed').length,
        leftOnly: buildDiffRows.filter(r => r.result.type === 'left-only').length,
        rightOnly: buildDiffRows.filter(r => r.result.type === 'right-only').length,
    }), [buildDiffRows]);

    // Shared expanded state for arrays, keyed by row index
    const [expandedKeys, setExpandedKeys] = useState<Set<number>>(new Set());

    const toggleExpanded = (idx: number) => setExpandedKeys(prev => {
        const next = new Set(prev);
        next.has(idx) ? next.delete(idx) : next.add(idx);
        return next;
    });

    const isExpanded = (idx: number) => expandedKeys.has(idx);

    // Render a JSON value with syntax highlighting
    const renderJsonValue = (val: any, depth: number = 0): React.ReactNode => {
        if (val === null || val === undefined) return <span className="text-orange-500">null</span>;
        if (typeof val === 'string') return <span className="text-green-700 dark:text-green-400">{JSON.stringify(val)}</span>;
        if (typeof val === 'number') return <span className="text-blue-600 dark:text-blue-400">{val}</span>;
        if (typeof val === 'boolean') return <span className="text-purple-600 dark:text-purple-400">{String(val)}</span>;
        if (Array.isArray(val)) {
            if (val.length === 0) return <span className="text-gray-400">[]</span>;
            return <span className="text-gray-500 dark:text-gray-400">[{val.length} items]</span>;
        }
        if (typeof val === 'object') {
            const keys = Object.keys(val);
            if (keys.length === 0) return <span className="text-gray-400">{'{}'}</span>;
            if (depth > 1) return <span className="text-gray-500 dark:text-gray-400">{'{'}{keys.length} keys{'}'}</span>;
            return (
                <>
                    {'{'}
                    {keys.map((k, idx) => (
                        <span key={k}>
                            {'\n'}{'  '.repeat(depth + 1)}<span className="text-blue-700 dark:text-blue-400">"{k}"</span>: {renderJsonValue(val[k], depth + 1)}{idx < keys.length - 1 ? ',' : ''}
                        </span>
                    ))}
                    {'\n'}{'  '.repeat(depth)}{'}'}
                </>
            );
        }
        return null;
    };

    // Expandable array renderer with per-item diff comparison
    const RenderArray: React.FC<{ arr: any[]; otherArr?: any[]; rowIdx: number; depth: number }> = ({ arr, otherArr, rowIdx, depth }) => {
        const expanded = isExpanded(rowIdx);

        return (
            <span>
                <span className="cursor-pointer select-none text-gray-400" onClick={() => toggleExpanded(rowIdx)}>
                    [
                </span>
                {!expanded && (
                    <span className="cursor-pointer select-none text-gray-500 dark:text-gray-400" onClick={() => toggleExpanded(rowIdx)}>
                        {'...'} {arr.length} items]
                    </span>
                )}
                {expanded && (
                    <>
                        {arr.map((item, i) => {
                            const isObj = item && typeof item === 'object' && !Array.isArray(item);
                            const otherItem = otherArr && i < otherArr.length ? otherArr[i] : undefined;
                            const hasOther = otherArr !== undefined && i < (otherArr?.length ?? 0);
                            const itemSame = hasOther && deepEqual(item, otherItem);
                            const itemBg = !hasOther ? 'bg-red-100/50 dark:bg-red-900/30' : itemSame ? '' : 'bg-yellow-50/50 dark:bg-yellow-900/10';

                            return (
                                <span key={i} className={itemBg}>
                                    {'\n'}{'  '.repeat(depth + 1)}
                                    {isObj ? (
                                        <>
                                            <span className="text-gray-500 dark:text-gray-400 text-[8px] mr-1">[{i}]</span>
                                            {!itemSame && !hasOther && <span className="text-red-500 text-[8px] mr-1">removed</span>}
                                            {!itemSame && hasOther && <span className="text-yellow-600 dark:text-yellow-400 text-[8px] mr-1">diff</span>}
                                            {'{'}
                                            {Object.keys(item).map(k => (
                                                <span key={k}>
                                                    {'\n'}{'  '.repeat(depth + 2)}<span className="text-blue-700 dark:text-blue-400">"{k}"</span>: {renderJsonValue(item[k], depth + 2)}
                                                </span>
                                            ))}
                                            {'\n'}{'  '.repeat(depth + 1)}{'}'}{i < arr.length - 1 ? ',' : ''}
                                        </>
                                    ) : (
                                        <>
                                            {renderJsonValue(item, depth + 1)}{i < arr.length - 1 ? ',' : ''}
                                        </>
                                    )}
                                </span>
                            );
                        })}
                        {'\n'}{'  '.repeat(depth)}]
                    </>
                )}
            </span>
        );
    };

    const rowBg = (t: DiffResult) => {
        switch (t.type) {
            case 'same': return 'bg-transparent';
            case 'changed': return 'bg-yellow-100/60 dark:bg-yellow-900/20';
            case 'left-only': return 'bg-red-50 dark:bg-red-900/20';
            case 'right-only': return 'bg-green-50 dark:bg-green-900/20';
        }
    };

    return (
        <div>
            <div className="flex items-center gap-2 mb-1">
                <span className="text-[10px] font-bold text-gray-500">{leftLabel}</span>
                <span className="text-[9px] text-gray-400">vs</span>
                <span className="text-[10px] font-bold text-green-600 dark:text-green-400">{rightLabel}</span>
                <span className="ml-auto text-[9px] text-gray-400">
                    {stats.changed} key差异 · {stats.leftOnly} 仅左侧 · {stats.rightOnly} 仅右侧
                </span>
            </div>
            <div className="flex rounded-lg border border-gray-200 dark:border-base-300 overflow-hidden bg-gray-50 dark:bg-base-300 max-h-[600px]">
                {/* Left panel */}
                <div className="flex-1 overflow-auto border-r border-gray-200 dark:border-base-300" ref={leftRef} onScroll={onSyncScroll('left')}>
                    <div className="text-[9px] font-mono whitespace-pre">
                        {buildDiffRows.map((row, i) => (
                            <div key={i} className={`px-2 py-0.5 ${rowBg(row.result)} ${row.result.type === 'right-only' ? 'opacity-30' : ''}`}>
                                {row.result.type === 'right-only' ? (
                                    <span className="text-gray-400 italic">(not present)</span>
                                ) : (
                                    <span>
                                        <span className="text-blue-700 dark:text-blue-400">"{row.key}"</span>
                                        <span className="text-gray-400">: </span>
                                        {Array.isArray(row.leftVal) ? (
                                            <RenderArray arr={row.leftVal} otherArr={row.result.type === 'changed' && Array.isArray(row.rightVal) ? row.rightVal : undefined} rowIdx={i} depth={0} />
                                        ) : (
                                            renderJsonValue(row.leftVal, 0)
                                        )}
                                        {row.result.type === 'left-only' && <span className="text-red-500 ml-1 font-bold">[-]</span>}
                                    </span>
                                )}
                            </div>
                        ))}
                    </div>
                </div>
                {/* Right panel */}
                <div className="flex-1 overflow-auto" ref={rightRef} onScroll={onSyncScroll('right')}>
                    <div className="text-[9px] font-mono whitespace-pre">
                        {buildDiffRows.map((row, i) => (
                            <div key={i} className={`px-2 py-0.5 ${rowBg(row.result)} ${row.result.type === 'left-only' ? 'opacity-30' : ''}`}>
                                {row.result.type === 'left-only' ? (
                                    <span className="text-gray-400 italic">(not present)</span>
                                ) : (
                                    <span>
                                        <span className="text-blue-700 dark:text-blue-400">"{row.key}"</span>
                                        <span className="text-gray-400">: </span>
                                        {Array.isArray(row.rightVal) ? (
                                            <RenderArray arr={row.rightVal} otherArr={row.result.type === 'changed' && Array.isArray(row.leftVal) ? row.leftVal : undefined} rowIdx={i} depth={0} />
                                        ) : (
                                            renderJsonValue(row.rightVal, 0)
                                        )}
                                        {row.result.type === 'right-only' && <span className="text-green-500 ml-1 font-bold">[+]</span>}
                                    </span>
                                )}
                            </div>
                        ))}
                    </div>
                </div>
            </div>
        </div>
    );
};

/** 可折叠/展开的 JSON 折叠视图 — 展开即原文，折叠显示 {...} / [...] */
const JsonTreeView: React.FC<{ data: any; title?: string }> = ({ data, title }) => {
    const [collapsedPaths, setCollapsedPaths] = useState<Set<string>>(new Set()); // 默认全部展开

    const togglePath = (path: string) => setCollapsedPaths(prev => {
        const next = new Set(prev);
        next.has(path) ? next.delete(path) : next.add(path);
        return next;
    });

    const allCollapsedPaths = (() => {
        const all = new Set<string>();
        const collect = (val: any, path: string) => {
            if (val && typeof val === 'object') {
                all.add(path);
                if (Array.isArray(val)) val.forEach((item, i) => collect(item, `${path}[${i}]`));
                else Object.keys(val).forEach(k => collect(val[k], `${path}.${k}`));
            }
        };
        collect(data, '');
        return all;
    })();

    const isAllCollapsed = collapsedPaths.size >= allCollapsedPaths.size * 0.9 && allCollapsedPaths.size > 0;

    const foldCls = 'cursor-pointer select-none text-[10px] font-mono';
    const btnCls = 'opacity-0 group-hover:opacity-100 transition-opacity text-gray-400 hover:text-blue-500 ml-1 text-[10px] leading-none';

    const renderValue = (val: any): React.ReactNode => {
        if (val === null || val === undefined) return <span className="text-orange-500">null</span>;
        if (typeof val === 'string') return <span className="text-green-700 dark:text-green-400">"{val}"</span>;
        if (typeof val === 'number') return <span className="text-blue-600 dark:text-blue-400">{val}</span>;
        if (typeof val === 'boolean') return <span className="text-purple-600 dark:text-purple-400">{String(val)}</span>;
        return null;
    };

    const indentText = (depth: number) => '  '.repeat(depth);

    /** Render a single value line with trailing comma */
    const commaAfter = (idx: number, total: number) => idx < total - 1 ? ',' : '';

    /** Render children of an object/array — each child with comma if not last */
    const renderChildren = (val: any, path: string, depth: number, isArr: boolean): React.ReactNode => {
        const keys = isArr ? val.map((_: any, i: number) => i) : Object.keys(val);
        return keys.map((k: number | string, idx: number) => {
            const childVal = isArr ? val[k as number] : val[String(k)];
            const childPath = `${path}${isArr ? `[${k}]` : `.${String(k)}`}`;
            const childIsArr = Array.isArray(childVal);
            const childIsObj = typeof childVal === 'object' && !childIsArr;
            const comma = commaAfter(idx, keys.length);

            if (childVal === null || childVal === undefined) {
                return (
                    <div key={k} className="whitespace-pre">
                        <span className="text-gray-400">{indentText(depth + 1)}</span>
                        {!isArr && <span className="text-blue-700 dark:text-blue-400">"{String(k)}": </span>}
                        <span className="text-orange-500">null</span>
                        <span className="text-gray-400">{comma}</span>
                    </div>
                );
            }
            if (!childIsArr && !childIsObj) {
                return (
                    <div key={k} className="whitespace-pre">
                        <span className="text-gray-400">{indentText(depth + 1)}</span>
                        {!isArr && <span className="text-blue-700 dark:text-blue-400">"{String(k)}": </span>}
                        {renderValue(childVal)}
                        <span className="text-gray-400">{comma}</span>
                    </div>
                );
            }

            // Child is array or object — key + bracket on same line
            const childBracket = childIsArr ? '[' : '{';
            const childKeys = childIsArr ? childVal.map((_: any, i: number) => i) : Object.keys(childVal);
            const childEmpty = childKeys.length === 0;
            const childCollapsed = collapsedPaths.has(childPath);

            if (childCollapsed) {
                return (
                    <div key={k} className="whitespace-pre group">
                        <span className="text-gray-400">{indentText(depth + 1)}</span>
                        {!isArr && <span className="text-blue-700 dark:text-blue-400">"{String(k)}": </span>}
                        <span onClick={() => togglePath(childPath)} className={`${foldCls} text-gray-500 dark:text-gray-400 hover:text-blue-500`}>
                            {childBracket}...{childIsArr ? ']' : '}'}
                        </span>
                        <span className="text-gray-400">{comma}</span>
                        <span onClick={() => togglePath(childPath)} className={btnCls} title="展开">{'▸'}</span>
                    </div>
                );
            }
            if (childEmpty) {
                return (
                    <div key={k} className="whitespace-pre">
                        <span className="text-gray-400">{indentText(depth + 1)}</span>
                        {!isArr && <span className="text-blue-700 dark:text-blue-400">"{String(k)}": </span>}
                        <span className="text-gray-400">{childBracket}{childIsArr ? ']' : '}'}</span>
                        <span className="text-gray-400">{comma}</span>
                    </div>
                );
            }

            // Normal: key + bracket on same line, children, closing bracket + comma
            return (
                <div key={k}>
                    <div className="whitespace-pre">
                        <span className="text-gray-400">{indentText(depth + 1)}</span>
                        {!isArr && <span className="text-blue-700 dark:text-blue-400">"{String(k)}": </span>}
                        <span onClick={() => togglePath(childPath)} className={`${foldCls} text-gray-500 dark:text-gray-400 hover:text-blue-500`}>{childBracket}</span>
                    </div>
                    {renderChildren(childVal, childPath, depth + 1, childIsArr)}
                    <div className="whitespace-pre">
                        <span className="text-gray-400">{indentText(depth + 1)}</span>
                        <span className="text-gray-400">{childIsArr ? ']' : '}'}</span>
                        <span className="text-gray-400">{comma}</span>
                        <span onClick={() => togglePath(childPath)} className={btnCls} title="折叠">{'▾'}</span>
                    </div>
                </div>
            );
        });
    };

    const renderNode = (val: any, path: string, depth: number): React.ReactNode => {
        if (val === null || val === undefined) return <span className="text-orange-500">null</span>;
        const isArr = Array.isArray(val);
        const isObj = typeof val === 'object' && !isArr;
        if (!isArr && !isObj) return renderValue(val);

        const bracket = isArr ? '[' : '{';
        const keys = isArr ? val.map((_: any, i: number) => i) : Object.keys(val);
        const isEmpty = keys.length === 0;
        const isCollapsed = collapsedPaths.has(path);

        if (isCollapsed) {
            return (
                <div className="whitespace-pre group">
                    <span className="text-gray-400">{indentText(depth)}</span>
                    <span onClick={() => togglePath(path)} className={`${foldCls} text-gray-500 dark:text-gray-400 hover:text-blue-500`}>
                        {bracket}...{isArr ? ']' : '}'}
                    </span>
                    <span onClick={() => togglePath(path)} className={btnCls} title="展开">{'▸'}</span>
                </div>
            );
        }
        if (isEmpty) {
            return (
                <div className="whitespace-pre">
                    <span className="text-gray-400">{indentText(depth)}</span>
                    <span className="text-gray-400">{bracket}{isArr ? ']' : '}'}</span>
                </div>
            );
        }

        // Root: opening bracket on its own line, then children, then closing bracket
        return (
            <>
                <div className="whitespace-pre">
                    <span className="text-gray-400">{indentText(depth)}</span>
                    <span onClick={() => togglePath(path)} className={`${foldCls} text-gray-500 dark:text-gray-400 hover:text-blue-500`}>{bracket}</span>
                    <span onClick={() => togglePath(path)} className={btnCls} title="折叠">{'▾'}</span>
                </div>
                {renderChildren(val, path, depth, isArr)}
                <div className="whitespace-pre">
                    <span className="text-gray-400">{indentText(depth)}</span>
                    <span className="text-gray-400">{isArr ? ']' : '}'}</span>
                </div>
            </>
        );
    };

    return (
        <div>
            {title && (
                <div className="flex items-center justify-between mb-1">
                    <span className="text-[10px] font-bold text-gray-400 uppercase">{title}</span>
                    <button
                        type="button"
                        className="text-[9px] text-gray-500 hover:text-blue-500 dark:text-gray-400 dark:hover:text-blue-300 px-1.5 py-0.5 rounded bg-gray-100 dark:bg-gray-700/20"
                        onClick={() => isAllCollapsed ? setCollapsedPaths(new Set()) : setCollapsedPaths(new Set(allCollapsedPaths))}
                    >
                        {isAllCollapsed ? '全部展开' : '全部折叠'}
                    </button>
                </div>
            )}
            <div className="text-[10px] font-mono leading-relaxed">
                {renderNode(data, '', 0)}
            </div>
        </div>
    );
};

interface ProxyRequestLog {
    id: string;
    timestamp: number;
    method: string;
    url: string;
    status: number;
    duration: number;
    model?: string;
    mapped_model?: string;
    error?: string;
    request_body?: string;
    response_body?: string;
    input_tokens?: number;
    output_tokens?: number;
    account_email?: string;
    provider_name?: string;      // 供应商名称（使用供应商通道时）
    protocol?: string;  // "openai" | "anthropic" | "gemini"
    upstream_protocol?: string; // 上游供应商协议
    upstream_model?: string;    // 实际发送给上游的模型名
    upstream_url?: string;      // 实际请求的上游 URL
    upstream_request_body?: string; // 发送给供应商的请求报文
    upstream_response_body?: string;// 供应商的响应报文
    in_flight?: boolean;        // 请求是否正在进行中
}

interface ProxyStats {
    total_requests: number;
    success_count: number;
    error_count: number;
}

interface ProxyMonitorProps {
    className?: string;
}

// Log Table Component
interface LogTableProps {
    logs: ProxyRequestLog[];
    loading: boolean;
    onLogClick: (log: ProxyRequestLog) => void;
    onResend: (log: ProxyRequestLog) => void;
    t: any;
    inFlightTick?: number;
}

const LogTable: React.FC<LogTableProps> = ({
    logs,
    loading,
    onLogClick,
    onResend,
    t,
    inFlightTick
}) => {
    // inFlightTick forces re-render for running duration display
    void inFlightTick;
    // Helper: format protocol badge
    const getProtocolBadge = (proto?: string) => {
        if (!proto) return null;
        const p = proto.toLowerCase();
        const label = p === 'openai' ? 'OpenAI' : p === 'anthropic' ? 'Claude' : p === 'gemini' ? 'Gemini' : proto;
        const color = p === 'openai' ? 'bg-green-500' : p === 'anthropic' ? 'bg-orange-500' : p === 'gemini' ? 'bg-blue-500' : 'bg-gray-400';
        return <span className={`badge badge-xs text-white border-none ${color}`}>{label}</span>;
    };

    // Helper: truncate path to last segment(s)
    const shortPath = (s?: string, maxLen = 30) => {
        if (!s) return '-';
        return s.length > maxLen ? s.slice(0, maxLen) + '…' : s;
    };

    // Helper: extract path from URL
    const urlPath = (url?: string) => {
        if (!url) return '-';
        try { const u = new URL(url); return u.pathname + u.search; } catch { return url; }
    };

    return (
        <div
            className="flex-1 overflow-y-auto overflow-x-auto bg-white dark:bg-base-100"
        >
            <table className="table table-xs w-full">
                <thead className="bg-gray-50 dark:bg-base-200 text-gray-500 sticky top-0 z-10">
                    <tr>
                        <th style={{ width: '56px' }}>状态</th>
                        <th style={{ width: '56px' }}>方法</th>
                        <th style={{ width: '66px' }}>请求协议</th>
                        <th style={{ width: '130px' }}>模型</th>
                        <th style={{ width: '130px' }}>映射模型</th>
                        <th style={{ width: '120px' }}>供应商</th>
                        <th style={{ width: '76px' }}>供应商协议</th>
                        <th style={{ width: '110px' }}>上游模型</th>
                        <th style={{ width: '130px' }}>原始路径</th>
                        <th style={{ width: '140px' }}>目标路径</th>
                        <th className="text-right" style={{ width: '88px' }}>用量</th>
                        <th className="text-right" style={{ width: '76px' }}>耗时</th>
                        <th className="text-right" style={{ width: '76px' }}>时间</th>
                        <th style={{ width: '36px' }}></th>
                    </tr>
                </thead>
                <tbody className="font-mono text-gray-700 dark:text-gray-300">
                    {logs.map((log) => (
                        <tr
                            key={log.id}
                            className={`hover:bg-blue-50 dark:hover:bg-blue-900/20 cursor-pointer ${log.in_flight ? 'bg-yellow-50/60 dark:bg-yellow-900/10' : ''}`}
                            onClick={() => onLogClick(log)}
                        >
                            <td style={{ width: '56px' }}>
                                {log.in_flight && log.status === 0 ? (
                                    <span className="badge badge-xs badge-warning gap-1">
                                        <span className="loading loading-spinner loading-[8px]"></span>
                                        处理中
                                    </span>
                                ) : (
                                    <span className={`badge badge-xs text-white border-none ${log.status >= 200 && log.status < 400 ? 'badge-success' : 'badge-error'}`}>
                                        {log.status}
                                    </span>
                                )}
                            </td>
                            <td className="font-bold" style={{ width: '56px' }}>{log.method}</td>
                            <td style={{ width: '66px' }}>{getProtocolBadge(log.protocol)}</td>
                            <td className="text-blue-600 truncate text-[11px]" style={{ width: '130px', maxWidth: '130px' }} title={log.model || ''}>
                                {log.model || '-'}
                            </td>
                            <td className="text-purple-600 dark:text-purple-400 truncate text-[10px]" style={{ width: '130px', maxWidth: '130px' }} title={log.mapped_model || ''}>
                                {log.mapped_model || '-'}
                            </td>
                            <td className="truncate text-[10px]" style={{ width: '120px', maxWidth: '120px' }} title={log.provider_name || log.account_email || ''}>
                                {log.provider_name ? (
                                    <span className="px-1 py-0.5 rounded bg-blue-100 dark:bg-blue-900/40 text-blue-700 dark:text-blue-300 text-[10px] font-medium">
                                        {log.provider_name}
                                    </span>
                                ) : log.account_email ? (
                                    <span className="text-gray-600 dark:text-gray-400">
                                        {log.account_email.replace(/(.{3}).*(@.*)/, '$1***$2')}
                                    </span>
                                ) : '-'}
                            </td>
                            <td style={{ width: '76px' }}>
                                {log.upstream_protocol ? getProtocolBadge(log.upstream_protocol) : <span className="text-gray-400">-</span>}
                            </td>
                            <td className="truncate text-[10px]" style={{ width: '110px', maxWidth: '110px' }} title={log.upstream_model || ''}>
                                {log.upstream_model || <span className="text-gray-400">-</span>}
                            </td>
                            <td className="truncate text-[10px]" style={{ width: '130px', maxWidth: '130px' }} title={log.url || ''}>
                                {shortPath(log.url, 30)}
                            </td>
                            <td className="truncate text-[10px]" style={{ width: '140px', maxWidth: '140px' }} title={log.upstream_url || ''}>
                                {log.upstream_url ? (
                                    <span className="text-emerald-600 dark:text-emerald-400">{shortPath(urlPath(log.upstream_url), 32)}</span>
                                ) : <span className="text-gray-400">-</span>}
                            </td>
                            <td className="text-right text-[9px]" style={{ width: '88px' }}>
                                {log.input_tokens != null && <div>I: {formatCompactNumber(log.input_tokens)}</div>}
                                {log.output_tokens != null && <div>O: {formatCompactNumber(log.output_tokens)}</div>}
                            </td>
                            <td className="text-right text-[11px]" style={{ width: '76px' }}>
                                {log.in_flight && log.status === 0 ? (
                                    <span className="text-yellow-600 dark:text-yellow-400 animate-pulse">
                                        {Math.floor((Date.now() - log.timestamp) / 1000)}s
                                    </span>
                                ) : (
                                    log.duration + 'ms'
                                )}
                            </td>
                            <td className="text-right text-[10px]" style={{ width: '76px' }}>
                                {new Date(log.timestamp).toLocaleTimeString('en-GB', { hour: '2-digit', minute: '2-digit', second: '2-digit' })}
                            </td>
                            <td style={{ width: '36px' }}>
                                <button
                                    type="button"
                                    className="btn btn-ghost btn-xs p-0.5 text-gray-400 hover:text-blue-500 dark:hover:text-blue-400"
                                    title="重新发送此请求"
                                    onClick={(e) => {
                                        e.stopPropagation();
                                        onResend(log);
                                    }}
                                >
                                    <Repeat2 size={14} />
                                </button>
                            </td>
                        </tr>
                    ))}
                </tbody>
            </table>

            {/* Loading indicator */}
            {loading && (
                <div className="flex items-center justify-center p-4 bg-white dark:bg-base-100">
                    <div className="loading loading-spinner loading-md"></div>
                    <span className="ml-3 text-sm text-gray-500">{t('common.loading')}</span>
                </div>
            )}

            {/* Empty state */}
            {!loading && logs.length === 0 && (
                <div className="flex items-center justify-center p-8 text-gray-400">
                    {t('monitor.table.empty') || '暂无请求记录'}
                </div>
            )}
        </div>
    );
};


// Visual View Component for structured request/response display
const VisualView: React.FC<{ log: ProxyRequestLog }> = ({ log }) => {
    const [expandedSections, setExpandedSections] = useState<Record<string, boolean>>({
        request_messages: true,
        request_system: false,
        request_tools: false,
        request_params: false,
        response_content: true,
        response_usage: false,
        response_meta: false,
    });

    const toggleSection = (key: string) => {
        setExpandedSections(prev => ({ ...prev, [key]: !prev[key] }));
    };

    const safeParse = (body?: string): any => {
        if (!body) return null;
        try { return JSON.parse(body); } catch { return null; }
    };

    /**
     * Parse SSE stream (Anthropic format) into a reconstructed response object.
     * Handles: message_start, content_block_start, content_block_delta, message_delta, message_stop
     */
    const parseSSE = (body?: string): any | null => {
        if (!body || !body.includes('event:')) return null;

        let model = '';
        let messageId = '';
        let stopReason = '';
        const contentBlocks: any[] = [];
        let inputTokens = 0;
        let outputTokens = 0;
        let cacheCreationTokens = 0;
        let cacheReadTokens = 0;
        let error: string | null = null;

        const events = body.split('\n\n').filter(Boolean);
        for (const event of events) {
            const lines = event.split('\n');
            const eventLine = lines.find(l => l.startsWith('event:'));
            const dataLine = lines.find(l => l.startsWith('data:'));
            if (!eventLine || !dataLine) continue;

            const eventType = eventLine.slice(6);
            const dataStr = dataLine.slice(5);
            let data: any;
            try { data = JSON.parse(dataStr); } catch { continue; }

            switch (eventType) {
                case 'message_start':
                    model = data.message?.model || '';
                    messageId = data.message?.id || '';
                    inputTokens = data.message?.usage?.input_tokens || 0;
                    outputTokens = data.message?.usage?.output_tokens || 0;
                    break;
                case 'content_block_start':
                    contentBlocks.push({
                        index: data.index,
                        type: data.content_block?.type || 'unknown',
                        text: data.content_block?.text || '',
                        thinking: data.content_block?.thinking || '',
                        name: data.content_block?.name,
                        input: data.content_block?.input,
                    });
                    break;
                case 'content_block_delta':
                    const idx = data.index ?? -1;
                    const block = contentBlocks.find(b => b.index === idx);
                    if (block) {
                        if (data.delta?.type === 'text_delta') {
                            block.text += data.delta.text || '';
                        } else if (data.delta?.type === 'thinking_delta') {
                            block.thinking += data.delta.thinking || '';
                        } else if (data.delta?.type === 'signature_delta') {
                            block.signature = (block.signature || '') + (data.delta.signature || '');
                        } else if (data.delta?.type === 'input_json_delta') {
                            block.inputJson = (block.inputJson || '') + (data.delta.partial_json || '');
                        }
                    }
                    break;
                case 'message_delta':
                    stopReason = data.delta?.stop_reason || stopReason;
                    outputTokens = data.usage?.output_tokens ?? outputTokens;
                    if (data.usage) {
                        if (data.usage.input_tokens != null) inputTokens = data.usage.input_tokens;
                        if (data.usage.cache_creation_input_tokens != null) cacheCreationTokens = data.usage.cache_creation_input_tokens;
                        if (data.usage.cache_read_input_tokens != null) cacheReadTokens = data.usage.cache_read_input_tokens;
                    }
                    break;
                case 'error':
                    error = data.error?.message || JSON.stringify(data.error);
                    break;
            }
        }

        if (!model && !contentBlocks.length && !error) return null;

        return {
            type: 'message',
            model,
            id: messageId,
            role: 'assistant',
            content: contentBlocks.map(b => {
                if (b.type === 'text') return { type: 'text', text: b.text };
                if (b.type === 'thinking') return { type: 'thinking', thinking: b.thinking, signature: b.signature };
                if (b.type === 'tool_use') return { type: 'tool_use', name: b.name, input: b.input || (() => { try { return JSON.parse(b.inputJson || '{}'); } catch { return {}; } })() };
                return { type: b.type, ...b };
            }),
            stop_reason: stopReason,
            usage: {
                input_tokens: inputTokens,
                output_tokens: outputTokens,
                cache_creation_input_tokens: cacheCreationTokens || undefined,
                cache_read_input_tokens: cacheReadTokens || undefined,
            },
            _error: error,
        };
    };

    const formatBytes = (bytes: number): string => {
        if (bytes < 1024) return `${bytes} B`;
        if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
        return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
    };

    const byteSize = (value: unknown): number => {
        if (value == null) return 0;
        if (typeof value === 'string') return new Blob([value]).size;
        return new Blob([JSON.stringify(value)]).size;
    };

    const requestObj = safeParse(log.request_body);
    let responseObj = safeParse(log.response_body);
    // If response is not valid JSON, try parsing as SSE stream
    if (!responseObj && log.response_body && log.response_body.includes('event:')) {
        responseObj = parseSSE(log.response_body);
    }
    const isSSEResponse = log.response_body?.includes('event:') && !safeParse(log.response_body);
    const protocol = log.protocol || 'anthropic';

    // Flow diagram data
    const modelMap = log.mapped_model && log.model !== log.mapped_model
        ? `${log.model} => ${log.mapped_model}`
        : (log.model || '-');
    const upstreamName = log.provider_name || log.account_email?.replace(/(.{3}).*(@.*)/, '$1***$2') || '-';
    const flowInfo = [
        log.protocol && `协议:${log.protocol}`,
        log.upstream_model && `上游:${log.upstream_model}`,
        log.upstream_protocol && `上游协议:${log.upstream_protocol}`,
    ].filter(Boolean).join(' · ');

    const SectionCard: React.FC<{
        icon: React.ReactNode;
        title: string;
        sectionKey: string;
        sizeBytes?: number;
        children: React.ReactNode;
    }> = ({ icon, title, sizeBytes, sectionKey, children }) => {
        const handleToggle = (e: React.MouseEvent) => {
            // Find and save scroll positions of all scrollable ancestors
            const scrollableEls: { el: Element; top: number }[] = [];
            let node: Element | null = e.currentTarget;
            while (node) {
                if (node.scrollHeight > node.clientHeight) {
                    scrollableEls.push({ el: node, top: node.scrollTop });
                }
                node = node.parentElement;
            }
            toggleSection(sectionKey);
            // Restore scroll positions after React re-render
            requestAnimationFrame(() => {
                for (const { el, top } of scrollableEls) {
                    (el as HTMLElement).scrollTop = top;
                }
            });
        };
        return (
            <div className="border border-gray-100 dark:border-base-300 rounded-lg overflow-hidden">
                <div
                    role="button"
                    tabIndex={-1}
                    className="w-full flex items-center gap-2 px-3 py-2 bg-gray-50 dark:bg-base-200 hover:bg-gray-100 dark:hover:bg-base-300 transition-colors cursor-pointer select-none"
                    onClick={handleToggle}
                    onMouseDown={(e) => { e.preventDefault(); e.stopPropagation(); }}
                    onKeyDown={(e) => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); toggleSection(sectionKey); } }}
                >
                {icon}
                <span className="text-xs font-bold text-gray-700 dark:text-gray-300">{title}</span>
                {sizeBytes != null && sizeBytes > 0 && (
                    <span className="text-[9px] font-mono text-gray-400 dark:text-gray-500 bg-gray-100 dark:bg-base-300 px-1.5 py-0.5 rounded">{formatBytes(sizeBytes)}</span>
                )}
                <span className="ml-auto">
                    {expandedSections[sectionKey] ? <ChevronUp size={14} className="text-gray-400" /> : <ChevronDown size={14} className="text-gray-400" />}
                </span>
            </div>
            {expandedSections[sectionKey] && (
                <div className="p-3 bg-white dark:bg-base-100">{children}</div>
            )}
        </div>
    );
    };

    // Collapsible tool block for conversation messages — shows preview, expand for full
    const ToolBlock: React.FC<{ title: string; color: string; content: string }> = ({ title, color, content }) => {
        const [open, setOpen] = useState(false);
        const pretty = (() => { try { return JSON.stringify(typeof content === 'string' ? JSON.parse(content) : content, null, 2); } catch { return String(content); } })();
        const lines = pretty.split('\n');
        const isLong = lines.length > 4;
        const preview = isLong ? lines.slice(0, 3).join('\n') : pretty;

        return (
            <div className="rounded-lg border border-gray-200 dark:border-base-300">
                <div className={`flex items-center justify-between px-2 py-1 border-b border-gray-100 dark:border-base-300 ${color}`}>
                    <span className="text-[10px] font-medium">{title}</span>
                    {isLong && (
                        <button
                            onClick={() => setOpen(!open)}
                            className="text-[9px] font-medium opacity-70 hover:opacity-100 transition-opacity"
                        >
                            {open ? '收起' : `展开完整 (${lines.length} 行)`}
                        </button>
                    )}
                </div>
                <pre className="text-[10px] font-mono text-gray-600 dark:text-gray-400 whitespace-pre-wrap break-all p-2 bg-gray-500/5 dark:bg-white/5">
                    {open ? pretty : preview}
                    {!open && isLong && <span className="text-gray-400 dark:text-gray-500 ml-1">...</span>}
                </pre>
            </div>
        );
    };

    // Parse request messages
    const renderRequestMessages = () => {
        if (protocol === 'openai' && requestObj?.messages) {
            return (requestObj.messages as any[]).map((msg: any, i: number) => {
                const isUser = msg.role === 'user';
                return (
                    <div key={i} className={`mb-2 last:mb-0 flex ${isUser ? 'justify-end' : 'justify-start'}`}>
                        <div className={`max-w-[80%] ${isUser ? 'flex flex-col items-end' : 'flex flex-col items-start'}`}>
                            <span className={`text-[9px] font-bold mb-0.5 ${isUser ? 'text-blue-600 dark:text-blue-400' : 'text-green-600 dark:text-green-400'}`}>
                                {msg.role}
                            </span>
                            {typeof msg.content === 'string' ? (
                                <TruncatableText text={msg.content} maxLength={300} className={`text-[10px] font-mono break-all whitespace-pre-wrap rounded-lg px-2.5 py-1.5 ${isUser ? 'bg-blue-500 text-white' : 'bg-gray-100 dark:bg-base-200 text-gray-600 dark:text-gray-400'}`} as="p" />
                            ) : Array.isArray(msg.content) ? (
                                <div className="space-y-1 w-full">
                                    {msg.content.map((part: any, j: number) => (
                                        <div key={j}>
                                            {part.type === 'text' && <TruncatableText text={part.text} maxLength={300} className={`text-[10px] font-mono break-all whitespace-pre-wrap rounded-lg px-2.5 py-1.5 ${isUser ? 'bg-blue-500 text-white' : 'bg-gray-100 dark:bg-base-200 text-gray-600 dark:text-gray-400'}`} as="p" />}
                                            {part.type === 'image_url' && <span className="text-[10px] text-amber-600 dark:text-amber-400">[Image]</span>}
                                            {part.type === 'tool_use' && (
                                                <ToolBlock
                                                    title={`[Tool: ${part.name}]`}
                                                    color="text-purple-600 dark:text-purple-400"
                                                    content={JSON.stringify(part.input ?? {})}
                                                />
                                            )}
                                            {part.type === 'tool_result' && (
                                                <ToolBlock
                                                    title="[Tool Result]"
                                                    color="text-cyan-600 dark:text-cyan-400"
                                                    content={typeof part.content === 'string' ? part.content : JSON.stringify(part.content)}
                                                />
                                            )}
                                        </div>
                                    ))}
                                </div>
                            ) : null}
                        </div>
                    </div>
                );
            });
        }

        if ((protocol === 'anthropic') && requestObj?.messages) {
            return (requestObj.messages as any[]).map((msg: any, i: number) => {
                const isUser = msg.role === 'user';
                return (
                    <div key={i} className={`mb-2 last:mb-0 flex ${isUser ? 'justify-end' : 'justify-start'}`}>
                        <div className={`max-w-[80%] ${isUser ? 'flex flex-col items-end' : 'flex flex-col items-start'}`}>
                            <span className={`text-[9px] font-bold mb-0.5 ${isUser ? 'text-blue-600 dark:text-blue-400' : 'text-green-600 dark:text-green-400'}`}>
                                {msg.role}
                            </span>
                            {Array.isArray(msg.content) ? (
                                <div className="space-y-1 w-full">
                                    {msg.content.map((part: any, j: number) => (
                                        <div key={j}>
                                            {part.type === 'text' && <TruncatableText text={part.text} maxLength={300} className={`text-[10px] font-mono break-all whitespace-pre-wrap rounded-lg px-2.5 py-1.5 ${isUser ? 'bg-blue-500 text-white' : 'bg-gray-100 dark:bg-base-200 text-gray-600 dark:text-gray-400'}`} as="p" />}
                                            {part.type === 'image' && <span className="text-[10px] text-amber-600 dark:text-amber-400">[Image: {part.source?.type}]</span>}
                                            {part.type === 'tool_use' && (
                                                <ToolBlock
                                                    title={`[Tool: ${part.name}]`}
                                                    color="text-purple-600 dark:text-purple-400"
                                                    content={JSON.stringify(part.input ?? {})}
                                                />
                                            )}
                                            {part.type === 'tool_result' && (
                                                <ToolBlock
                                                    title="[Tool Result]"
                                                    color="text-cyan-600 dark:text-cyan-400"
                                                    content={typeof part.content === 'string' ? part.content : JSON.stringify(part.content)}
                                                />
                                            )}
                                        </div>
                                    ))}
                                </div>
                            ) : typeof msg.content === 'string' ? (
                                <TruncatableText text={msg.content} maxLength={300} className={`text-[10px] font-mono break-all whitespace-pre-wrap rounded-lg px-2.5 py-1.5 ${isUser ? 'bg-blue-500 text-white' : 'bg-gray-100 dark:bg-base-200 text-gray-600 dark:text-gray-400'}`} as="p" />
                            ) : null}
                        </div>
                    </div>
                );
            });
        }

        return <p className="text-[10px] text-gray-400 italic">No messages found</p>;
    };

    const renderRequestSystem = () => {
        if (protocol === 'openai') {
            const sysMsg = requestObj?.messages?.find((m: any) => m.role === 'system');
            if (!sysMsg) return <p className="text-[10px] text-gray-400 italic">No system prompt</p>;
            return <TruncatableText text={typeof sysMsg.content === 'string' ? sysMsg.content : JSON.stringify(sysMsg.content)} maxLength={500} className="text-[10px] font-mono text-gray-600 dark:text-gray-400 whitespace-pre-wrap break-all" as="pre" />;
        }
        const sys = requestObj?.system;
        if (!sys) return <p className="text-[10px] text-gray-400 italic">No system prompt</p>;
        if (typeof sys === 'string') return <TruncatableText text={sys} maxLength={500} className="text-[10px] font-mono text-gray-600 dark:text-gray-400 whitespace-pre-wrap break-all" as="pre" />;
        if (Array.isArray(sys)) return sys.map((s: any, i: number) => (
            <TruncatableText key={i} text={s.text || JSON.stringify(s)} maxLength={500} className="text-[10px] font-mono text-gray-600 dark:text-gray-400 whitespace-pre-wrap break-all mb-1 last:mb-0" as="pre" />
        ));
        return <TruncatableText text={JSON.stringify(sys)} maxLength={500} className="text-[10px] font-mono text-gray-600 dark:text-gray-400 whitespace-pre-wrap break-all" as="pre" />;
    };

    const renderRequestTools = () => {
        const tools = requestObj?.tools;
        if (!tools || !Array.isArray(tools) || tools.length === 0) return <p className="text-[10px] text-gray-400 italic">No tools</p>;
        return (
            <div className="space-y-1">
                {tools.map((tool: any, i: number) => (
                    <div key={i} className="text-[10px]">
                        <span className="font-mono font-bold text-purple-600 dark:text-purple-400">{tool.name || (tool.function?.name)}</span>
                        {tool.description && <TruncatableText text={tool.description} maxLength={100} className="text-gray-500 dark:text-gray-400 ml-1" as="span" />}
                    </div>
                ))}
            </div>
        );
    };

    const renderRequestParams = () => {
        const excludeKeys = new Set(['messages', 'system', 'tools', 'model', 'request_body', 'response_body']);
        const params = Object.entries(requestObj || {}).filter(([k]) => !excludeKeys.has(k));
        if (params.length === 0) return <p className="text-[10px] text-gray-400 italic">No additional params</p>;
        return (
            <div className="space-y-1">
                {params.map(([k, v]) => (
                    <div key={k} className="flex items-baseline gap-2 text-[10px]">
                        <span className="font-mono font-bold text-gray-500 dark:text-gray-400 w-28 shrink-0">{k}</span>
                        <span className="font-mono text-gray-700 dark:text-gray-300">{typeof v === 'object' ? JSON.stringify(v) : String(v)}</span>
                    </div>
                ))}
            </div>
        );
    };

    const renderResponseContent = () => {
        if (!responseObj) return <p className="text-[10px] text-gray-400 italic">No response data</p>;

        // OpenAI format
        if (protocol === 'openai' && responseObj.choices) {
            return responseObj.choices.map((choice: any, i: number) => (
                <div key={i} className="mb-2 last:mb-0">
                    <div className="text-[9px] text-gray-400 mb-1">Choice #{i} {choice.finish_reason && `(finish: ${choice.finish_reason})`}</div>
                    {choice.message?.content && (
                        <TruncatableText text={choice.message.content} maxLength={500} className="text-[10px] font-mono text-gray-700 dark:text-gray-300 whitespace-pre-wrap break-all" as="pre" />
                    )}
                    {choice.message?.tool_calls && (
                        <div className="text-[10px] text-purple-600 dark:text-purple-400">
                            [Tool Calls: {choice.message.tool_calls.map((tc: any) => tc.function?.name).join(', ')}]
                        </div>
                    )}
                </div>
            ));
        }

        // Anthropic format (or parsed SSE)
        if ((protocol === 'anthropic' || isSSEResponse) && responseObj.content) {
            // Handle both array (parsed SSE) and string (consolidated backend response)
            if (typeof responseObj.content === 'string') {
                return (
                    <div className="mb-2 last:mb-0">
                        <div className="text-[9px] text-gray-400 mb-1">Block #0: text</div>
                        <TruncatableText text={responseObj.content} maxLength={500} className="text-[10px] font-mono text-gray-700 dark:text-gray-300 whitespace-pre-wrap break-all" as="pre" />
                    </div>
                );
            }
            return (responseObj.content as any[]).map((block: any, i: number) => (
                <div key={i} className="mb-2 last:mb-0">
                    <div className="text-[9px] text-gray-400 mb-1">
                        Block #{i}: {block.type}
                        {block.type === 'tool_use' && ` (${block.name})`}
                    </div>
                    {block.type === 'text' && (
                        <TruncatableText text={block.text} maxLength={500} className="text-[10px] font-mono text-gray-700 dark:text-gray-300 whitespace-pre-wrap break-all" as="pre" />
                    )}
                    {block.type === 'tool_use' && (
                        <TruncatableText text={JSON.stringify(block.input)} maxLength={300} className="text-[10px] font-mono text-purple-600 dark:text-purple-400 whitespace-pre-wrap" as="pre" />
                    )}
                    {block.type === 'thinking' && (
                        <details className="text-[10px]">
                            <summary className="cursor-pointer text-amber-600 dark:text-amber-400 font-bold">[Thinking...]</summary>
                            <TruncatableText text={block.thinking} maxLength={500} className="text-[10px] font-mono text-gray-600 dark:text-gray-400 whitespace-pre-wrap break-all mt-1" as="pre" />
                        </details>
                    )}
                </div>
            ));
        }

        // Gemini format
        if (protocol === 'gemini' && responseObj.candidates) {
            return responseObj.candidates.map((candidate: any, i: number) => (
                <div key={i} className="mb-2 last:mb-0">
                    <div className="text-[9px] text-gray-400 mb-1">Candidate #{i}</div>
                    {candidate.content?.parts?.map((part: any, j: number) => (
                        <div key={j}>
                            {part.text && <TruncatableText text={part.text} maxLength={500} className="text-[10px] font-mono text-gray-700 dark:text-gray-300 whitespace-pre-wrap break-all" as="pre" />}
                            {part.functionCall && <div className="text-[10px] text-purple-600 dark:text-purple-400">[Function: {part.functionCall.name}]</div>}
                        </div>
                    ))}
                </div>
            ));
        }

        // Fallback
        return <TruncatableText text={JSON.stringify(responseObj, null, 2)} maxLength={500} className="text-[10px] font-mono text-gray-600 dark:text-gray-400 whitespace-pre-wrap break-all" as="pre" />;
    };

    const renderResponseUsage = () => {
        const usage = responseObj?.usage || (protocol === 'openai' ? responseObj?.usage : responseObj?.usage);
        if (!usage && !log.input_tokens && !log.output_tokens) return <p className="text-[10px] text-gray-400 italic">No usage data</p>;

        if (protocol === 'openai' && usage) {
            return (
                <div className="grid grid-cols-3 gap-3 text-[10px]">
                    <div><span className="text-gray-400">Prompt</span><div className="font-mono font-bold text-blue-600 dark:text-blue-400">{formatCompactNumber(usage.prompt_tokens)}</div></div>
                    <div><span className="text-gray-400">Completion</span><div className="font-mono font-bold text-green-600 dark:text-green-400">{formatCompactNumber(usage.completion_tokens)}</div></div>
                    <div><span className="text-gray-400">Total</span><div className="font-mono font-bold text-gray-600 dark:text-gray-400">{formatCompactNumber(usage.total_tokens)}</div></div>
                </div>
            );
        }

        if ((protocol === 'anthropic' || isSSEResponse) && usage) {
            return (
                <div className="grid grid-cols-2 sm:grid-cols-4 gap-3 text-[10px]">
                    <div><span className="text-gray-400">Input</span><div className="font-mono font-bold text-blue-600 dark:text-blue-400">{formatCompactNumber(usage.input_tokens)}</div></div>
                    <div><span className="text-gray-400">Output</span><div className="font-mono font-bold text-green-600 dark:text-green-400">{formatCompactNumber(usage.output_tokens)}</div></div>
                    {usage.cache_creation_input_tokens != null && (
                        <div><span className="text-gray-400">Cache Write</span><div className="font-mono font-bold text-purple-600 dark:text-purple-400">{formatCompactNumber(usage.cache_creation_input_tokens)}</div></div>
                    )}
                    {usage.cache_read_input_tokens != null && (
                        <div><span className="text-gray-400">Cache Read</span><div className="font-mono font-bold text-amber-600 dark:text-amber-400">{formatCompactNumber(usage.cache_read_input_tokens)}</div></div>
                    )}
                </div>
            );
        }

        return (
            <div className="grid grid-cols-2 gap-3 text-[10px]">
                <div><span className="text-gray-400">Input</span><div className="font-mono font-bold text-blue-600 dark:text-blue-400">{formatCompactNumber(log.input_tokens ?? 0)}</div></div>
                <div><span className="text-gray-400">Output</span><div className="font-mono font-bold text-green-600 dark:text-green-400">{formatCompactNumber(log.output_tokens ?? 0)}</div></div>
            </div>
        );
    };

    const renderResponseMeta = () => {
        const meta: { key: string; value: string }[] = [];

        if (protocol === 'anthropic' || isSSEResponse) {
            if (responseObj?.stop_reason) meta.push({ key: 'stop_reason', value: responseObj.stop_reason });
            if (responseObj?.stop_sequence) meta.push({ key: 'stop_sequence', value: responseObj.stop_sequence });
            if (responseObj?.model) meta.push({ key: 'model', value: responseObj.model });
            if (responseObj?.id) meta.push({ key: 'id', value: responseObj.id });
            if (isSSEResponse && responseObj?._error) meta.push({ key: 'error', value: responseObj._error });
        } else if (protocol === 'openai') {
            if (responseObj?.model) meta.push({ key: 'model', value: responseObj.model });
            if (responseObj?.id) meta.push({ key: 'id', value: responseObj.id });
            if (responseObj?.system_fingerprint) meta.push({ key: 'system_fingerprint', value: responseObj.system_fingerprint });
            if (responseObj?.choices?.[0]?.finish_reason) meta.push({ key: 'finish_reason', value: responseObj.choices[0].finish_reason });
        }

        if (log.error) meta.push({ key: 'error', value: log.error });

        if (meta.length === 0) return <p className="text-[10px] text-gray-400 italic">No metadata</p>;

        return (
            <div className="space-y-1">
                {meta.map(m => (
                    <div key={m.key} className="flex items-baseline gap-2 text-[10px]">
                        <span className="font-mono font-bold text-gray-500 dark:text-gray-400 w-28 shrink-0">{m.key}</span>
                        <span className={`font-mono ${m.key === 'error' ? 'text-red-600 dark:text-red-400' : 'text-gray-700 dark:text-gray-300'}`}>{m.value}</span>
                    </div>
                ))}
            </div>
        );
    };

    return (
        <div className="space-y-3">
            {/* Interaction Flow Diagram */}
            <div className="bg-gradient-to-r from-blue-50 to-indigo-50 dark:from-blue-900/10 dark:to-indigo-900/10 rounded-lg p-4 border border-blue-100 dark:border-blue-900/20">
                <div className="text-[10px] font-bold text-gray-500 dark:text-gray-400 uppercase tracking-wider mb-3">交互流程</div>
                <div className="flex flex-col sm:flex-row items-center gap-2 sm:gap-3">
                    <div className="flex items-center gap-2 bg-white dark:bg-base-100 px-3 py-2 rounded-lg shadow-sm border border-gray-100 dark:border-base-300">
                        <MessageSquare size={14} className="text-blue-500" />
                        <div>
                            <div className="text-[10px] font-bold text-gray-700 dark:text-gray-300">Client</div>
                            <div className="text-[9px] text-gray-400">{log.method} {log.url}</div>
                        </div>
                    </div>
                    <ArrowRight size={14} className="text-gray-300 dark:text-gray-600 rotate-90 sm:rotate-0" />
                    <div className="flex items-center gap-2 bg-white dark:bg-base-100 px-3 py-2 rounded-lg shadow-sm border border-gray-100 dark:border-base-300">
                        <Server size={14} className="text-orange-500" />
                        <div>
                            <div className="text-[10px] font-bold text-gray-700 dark:text-gray-300">Proxy</div>
                            <div className="text-[9px] text-gray-400">port 8150</div>
                        </div>
                    </div>
                    <ArrowRight size={14} className="text-gray-300 dark:text-gray-600 rotate-90 sm:rotate-0" />
                    <div className="flex items-center gap-2 bg-white dark:bg-base-100 px-3 py-2 rounded-lg shadow-sm border border-gray-100 dark:border-base-300">
                        <Cpu size={14} className="text-green-500" />
                        <div>
                            <div className="text-[10px] font-bold text-gray-700 dark:text-gray-300 truncate max-w-[140px]">{upstreamName}</div>
                            <TruncatableText text={modelMap} maxLength={20} className="text-[9px] text-gray-400" as="div" />
                            {flowInfo && <div className="text-[8px] text-gray-400 mt-0.5">{flowInfo}</div>}
                        </div>
                    </div>
                </div>
                <div className="flex items-center gap-3 mt-3 text-[9px] text-gray-400">
                    <span>{log.duration}ms</span>
                    <span>•</span>
                    <span>{log.status === 200 ? 'Success' : `Error (${log.status})`}</span>
                    {log.error && (
                        <span className="text-red-500 flex items-center gap-0.5"><AlertTriangle size={10} /> {log.error}</span>
                    )}
                </div>
            </div>

            {/* Request Section */}
            <div className="space-y-2">
                <div className="flex items-center gap-1.5">
                    <div className="w-2 h-2 rounded-full bg-blue-500"></div>
                    <span className="text-[10px] font-bold uppercase text-gray-500 dark:text-gray-400 tracking-wider">请求</span>
                </div>
                <SectionCard icon={<Cpu size={14} className="text-blue-500" />} title={`模型: ${log.model || '-'}`} sectionKey="request_messages" sizeBytes={byteSize(requestObj?.messages)}>
                    {renderRequestMessages()}
                </SectionCard>
                <SectionCard icon={<MessageSquare size={14} className="text-purple-500" />} title="System Prompt" sectionKey="request_system" sizeBytes={byteSize(requestObj?.system)}>
                    {renderRequestSystem()}
                </SectionCard>
                <SectionCard icon={<span className="text-[10px] font-bold text-orange-500">Fn</span>} title={`Tools (${requestObj?.tools?.length || 0})`} sectionKey="request_tools" sizeBytes={byteSize(requestObj?.tools)}>
                    {renderRequestTools()}
                </SectionCard>
                <SectionCard icon={<span className="text-[10px] font-bold text-gray-500">⚙</span>} title="Parameters" sectionKey="request_params" sizeBytes={byteSize(Object.fromEntries(Object.entries(requestObj || {}).filter(([k]) => !new Set(['messages', 'system', 'tools', 'model', 'request_body', 'response_body']).has(k))))}>
                    {renderRequestParams()}
                </SectionCard>
            </div>

            {/* Response Section */}
            <div className="space-y-2">
                <div className="flex items-center gap-1.5">
                    <div className={`w-2 h-2 rounded-full ${log.status >= 400 ? 'bg-red-500' : 'bg-green-500'}`}></div>
                    <span className="text-[10px] font-bold uppercase text-gray-500 dark:text-gray-400 tracking-wider">响应</span>
                    <span className={`px-1.5 py-0.5 rounded text-[9px] font-bold ${log.status >= 200 && log.status < 400 ? 'bg-green-100 text-green-700 dark:bg-green-900/40 dark:text-green-300' : 'bg-red-100 text-red-700 dark:bg-red-900/40 dark:text-red-300'}`}>{log.status}</span>
                </div>
                <SectionCard icon={<MessageSquare size={14} className="text-green-500" />} title="Response Content" sectionKey="response_content" sizeBytes={byteSize(responseObj?.content)}>
                    {renderResponseContent()}
                </SectionCard>
                <SectionCard icon={<span className="text-[10px] font-bold text-blue-500">Tok</span>} title="Token Usage" sectionKey="response_usage" sizeBytes={byteSize(responseObj?.usage)}>
                    {renderResponseUsage()}
                </SectionCard>
                <SectionCard icon={<span className="text-[10px] font-bold text-gray-500">#</span>} title="Metadata" sectionKey="response_meta" sizeBytes={byteSize(Object.fromEntries([['stop_reason', responseObj?.stop_reason], ['stop_sequence', responseObj?.stop_sequence], ['model', responseObj?.model], ['id', responseObj?.id], ['error', log.error]].filter(([, v]) => v != null).map(([k, v]) => [k, String(v)])))}>
                    {renderResponseMeta()}
                </SectionCard>
            </div>
        </div>
    );
};

export const ProxyMonitor: React.FC<ProxyMonitorProps> = ({ className }) => {
    const { t } = useTranslation();
    const [logs, setLogs] = useState<ProxyRequestLog[]>([]);
    const [stats, setStats] = useState<ProxyStats>({ total_requests: 0, success_count: 0, error_count: 0 });
    const [filter, setFilter] = useState('');
    const [accountFilter, setAccountFilter] = useState('');
    // [FIX] 使用 ref 存储最新的筛选条件，避免 setInterval 闭包问题
    const filterRef = useRef(filter);
    // Running timer for in-flight logs — forces re-render every second
    const [inFlightTick, setInFlightTick] = useState(0);
    const accountFilterRef = useRef(accountFilter);
    const currentPageRef = useRef(1);
    const [selectedLog, setSelectedLog] = useState<ProxyRequestLog | null>(null);
    const [detailViewMode, setDetailViewMode] = useState<'raw' | 'upstream' | 'visual' | 'compare'>('raw');

    // Reset view mode when dialog opens for a different log
    useEffect(() => {
        setDetailViewMode('raw');
    }, [selectedLog?.id]);
    const [isLoggingEnabled, setIsLoggingEnabled] = useState(false);
    const [isClearConfirmOpen, setIsClearConfirmOpen] = useState(false);
    const [copiedRequestId, setCopiedRequestId] = useState<string | null>(null);
    const [resendingId, setResendingId] = useState<string | null>(null);

    const { accounts, fetchAccounts } = useAccountStore();
    const llmLogging = useLlmLogging();
    const [llmLogViewerOpen, setLlmLogViewerOpen] = useState(false);

    // Initialize LLM logging status on mount
    useEffect(() => {
        llmLogging.loadStatus();
    }, []);

    // Pagination state
    const PAGE_SIZE_OPTIONS = [50, 100, 200, 500];
    const [pageSize, setPageSize] = useState(100);
    const [currentPage, setCurrentPage] = useState(1);
    const [totalCount, setTotalCount] = useState(0);
    const [loading, setLoading] = useState(false);
    const [loadingDetail, setLoadingDetail] = useState(false);

    const uniqueAccounts = useMemo(() => {
        const emailSet = new Set<string>();
        logs.forEach(log => {
            if (log.account_email) {
                emailSet.add(log.account_email);
            }
        });
        accounts.forEach(acc => {
            emailSet.add(acc.email);
        });
        return Array.from(emailSet).sort();
    }, [logs, accounts]);

    const loadData = async (page = 1, searchFilter = filter, accountEmailFilter = accountFilter) => {
        if (loading) return;
        setLoading(true);

        try {
            // Add timeout control (10 seconds)
            const timeoutPromise = new Promise((_, reject) =>
                setTimeout(() => reject(new Error('Request timeout')), 10000)
            );

            const config = await Promise.race([
                invoke<AppConfig>('load_config'),
                timeoutPromise
            ]) as AppConfig;

            if (config && config.proxy) {
                setIsLoggingEnabled(config.proxy.enable_logging);
                await invoke('set_proxy_monitor_enabled', { enabled: config.proxy.enable_logging });
            }

            const errorsOnly = searchFilter === '__ERROR__';
            const baseFilter = errorsOnly ? '' : searchFilter;
            const actualFilter = accountEmailFilter
                ? (baseFilter ? `${baseFilter} ${accountEmailFilter}` : accountEmailFilter)
                : baseFilter;

            // Get count with filter
            const count = await Promise.race([
                invoke<number>('get_proxy_logs_count_filtered', {
                    filter: actualFilter,
                    errorsOnly: errorsOnly
                }),
                timeoutPromise
            ]) as number;
            setTotalCount(count);

            // Use filtered paginated query
            const offset = (page - 1) * pageSize;
            const history = await Promise.race([
                invoke<ProxyRequestLog[]>('get_proxy_logs_filtered', {
                    filter: actualFilter,
                    errorsOnly: errorsOnly,
                    limit: pageSize,
                    offset: offset
                }),
                timeoutPromise
            ]) as ProxyRequestLog[];

            if (Array.isArray(history)) {
                setLogs(history);
                // Clear pending logs to avoid duplicates (database data is authoritative)
                pendingLogsRef.current = [];
            }

            const currentStats = await Promise.race([
                invoke<ProxyStats>('get_proxy_stats'),
                timeoutPromise
            ]) as ProxyStats;

            if (currentStats) setStats(currentStats);
        } catch (e: any) {
            console.error("Failed to load proxy data", e);
            if (e.message === 'Request timeout') {
                // Show timeout error to user
                console.error('Loading monitor data timeout, please try again later');
            }
        } finally {
            setLoading(false);
        }
    };

    const totalPages = Math.ceil(totalCount / pageSize);
    const pageStart = totalCount === 0 ? 0 : (currentPage - 1) * pageSize + 1;
    const pageEnd = totalCount === 0 ? 0 : Math.min(currentPage * pageSize, totalCount);

    const goToPage = (page: number) => {
        if (page >= 1 && page <= totalPages && page !== currentPage) {
            setCurrentPage(page);
            currentPageRef.current = page; // [FIX] 同步 ref
            loadData(page, filter, accountFilter);
        }
    };

    const toggleLogging = async () => {
        const newState = !isLoggingEnabled;
        try {
            const config = await invoke<AppConfig>('load_config');
            if (config && config.proxy) {
                config.proxy.enable_logging = newState;
                await invoke('save_config', { config });
                await invoke('set_proxy_monitor_enabled', { enabled: newState });
                setIsLoggingEnabled(newState);
            }
        } catch (e) {
            console.error("Failed to toggle logging", e);
        }
    };

    const pendingLogsRef = useRef<ProxyRequestLog[]>([]);
    const listenerSetupRef = useRef(false);
    const isMountedRef = useRef(true);

    // Running timer: tick every 1s while in-flight logs exist for live duration display
    useEffect(() => {
        const hasInFlight = logs.some(log => log.in_flight);
        if (!hasInFlight) return;
        const interval = setInterval(() => setInFlightTick(t => t + 1), 1000);
        return () => clearInterval(interval);
    }, [logs.some(log => log.in_flight)]);

    useEffect(() => {
        isMountedRef.current = true;
        loadData();
        fetchAccounts();

        let unlistenFn: (() => void) | null = null;
        let updateTimeout: ReturnType<typeof setTimeout> | null = null;

        const setupListener = async () => {
            if (!isTauri()) return;
            // Prevent duplicate listener registration (React 18 StrictMode)
            if (listenerSetupRef.current) {
                console.debug('[ProxyMonitor] Listener already set up, skipping...');
                return;
            }
            listenerSetupRef.current = true;

            console.debug('[ProxyMonitor] Setting up event listener for proxy://request');
            unlistenFn = await listen<ProxyRequestLog>('proxy://request', (event) => {
                if (!isMountedRef.current) return;

                const newLog = event.payload;

                // 移除 body 以减少内存占用
                const logSummary = {
                    ...newLog,
                    request_body: undefined,
                    response_body: undefined
                };

                // Check if a log with same ID already exists
                const existingIdx = pendingLogsRef.current.findIndex(log => log.id === newLog.id);
                if (existingIdx >= 0) {
                    const existing = pendingLogsRef.current[existingIdx];
                    // If existing is in_flight and new one is final, replace it
                    if (existing.in_flight && !newLog.in_flight) {
                        pendingLogsRef.current[existingIdx] = logSummary;
                        console.debug('[ProxyMonitor] Updated in-flight log with final:', newLog.id);
                    }
                    // Otherwise, ignore duplicate
                    return;
                }

                pendingLogsRef.current.push(logSummary);

                // 防抖:每 500ms 批量更新一次
                if (updateTimeout) clearTimeout(updateTimeout);
                updateTimeout = setTimeout(async () => {
                    if (!isMountedRef.current) return;

                    const currentPending = pendingLogsRef.current;
                    if (currentPending.length > 0) {
                        setLogs(prev => {
                            // Deduplicate by id — replace in_flight logs with final versions
                            const prevMap = new Map(prev.map(log => [log.id, log]));
                            for (const newLog of currentPending) {
                                const existing = prevMap.get(newLog.id);
                                if (!existing || (existing.in_flight && !newLog.in_flight)) {
                                    prevMap.set(newLog.id, newLog);
                                }
                            }
                            // Also add pending logs not in prev
                            const prevIds = new Set(prev.map(log => log.id));
                            for (const pLog of currentPending) {
                                if (!prevIds.has(pLog.id)) {
                                    prevMap.set(pLog.id, pLog);
                                }
                            }
                            // Merge and sort by timestamp descending (newest first)
                            const merged = Array.from(prevMap.values());
                            merged.sort((a, b) => b.timestamp - a.timestamp);
                            return merged.slice(0, 100);
                        });

                        // Fetch stats and total count from backend instead of local calculation
                        try {
                            const [currentStats, count] = await Promise.all([
                                invoke<ProxyStats>('get_proxy_stats'),
                                invoke<number>('get_proxy_logs_count_filtered', { filter: '', errorsOnly: false })
                            ]);
                            if (isMountedRef.current) {
                                if (currentStats) setStats(currentStats);
                                setTotalCount(count);
                            }
                        } catch (e) {
                            console.error('Failed to fetch stats:', e);
                        }

                        pendingLogsRef.current = [];
                    }
                }, 500);
            });
        };
        setupListener();

        // Web 模式補強：如果不是 Tauri 環境，則啟用定時輪詢
        let pollInterval: number | null = null;
        if (!isTauri()) {
            console.debug('[ProxyMonitor] Web mode detected, starting auto-poll (10s)');
            pollInterval = window.setInterval(() => {
                if (isMountedRef.current && !loading) {
                    // [FIX] 使用 ref.current 获取最新的筛选条件
                    loadData(currentPageRef.current, filterRef.current, accountFilterRef.current);
                }
            }, 10000);
        }

        return () => {
            isMountedRef.current = false;
            listenerSetupRef.current = false;
            if (unlistenFn) unlistenFn();
            if (updateTimeout) clearTimeout(updateTimeout);
            if (pollInterval) clearInterval(pollInterval);
        };
    }, []);

    useEffect(() => {
        setCopiedRequestId(null);
    }, [selectedLog?.id]);

    // Reload when pageSize changes
    useEffect(() => {
        setCurrentPage(1);
        loadData(1, filter, accountFilter);
    }, [pageSize]);

    // Reload when filter changes (search based on all logs)
    useEffect(() => {
        setCurrentPage(1);
        loadData(1, filter, accountFilter);
        // [FIX] 同步 ref 值，供 setInterval 使用
        filterRef.current = filter;
        accountFilterRef.current = accountFilter;
        currentPageRef.current = 1;
    }, [filter, accountFilter]);

    // Logs are already filtered and sorted by backend
    // Apply account filter on frontend
    const filteredLogs = useMemo(() => {
        if (!accountFilter) return logs;
        return logs.filter(log => log.account_email === accountFilter);
    }, [logs, accountFilter]);

    const quickFilters = [
        { label: t('monitor.filters.all'), value: '' },
        { label: t('monitor.filters.error'), value: '__ERROR__' },
        { label: t('monitor.filters.chat'), value: 'completions' },
        { label: t('monitor.filters.gemini'), value: 'gemini' },
        { label: t('monitor.filters.claude'), value: 'claude' },
        { label: t('monitor.filters.images'), value: 'images' }
    ];

    const clearLogs = () => {
        setIsClearConfirmOpen(true);
    };

    const getProxyBaseUrl = (): string => {
        // Use Vite proxy env if set (e.g. http://host.docker.internal:8150 for Docker)
        if (import.meta.env.VITE_API_PROXY_URL) {
            return import.meta.env.VITE_API_PROXY_URL as string;
        }
        // Default to the proxy port — matches vite.config.ts proxy target
        return 'http://127.0.0.1:8150';
    };

    const handleResend = async (log: ProxyRequestLog) => {
        // If request_body is not available (stripped in table view), try to fetch detail first
        let body = log.request_body;
        if (!body) {
            try {
                const detail = await invoke<ProxyRequestLog>('get_proxy_log_detail', { logId: log.id });
                body = detail?.request_body;
            } catch (e) {
                console.warn('Failed to fetch log detail for resend', e);
            }
        }
        if (!body) {
            console.warn('Cannot resend: request_body is not available');
            return;
        }
        setResendingId(log.id);
        try {
            const base = getProxyBaseUrl();
            const url = `${base}${log.url}`;
            const apiKey = sessionStorage.getItem('abv_admin_api_key');
            await fetch(url, {
                method: log.method,
                headers: {
                    'Content-Type': 'application/json',
                    ...(apiKey ? {
                        'Authorization': `Bearer ${apiKey}`,
                        'x-api-key': apiKey,
                    } : {}),
                },
                body,
            });
            // After resend, refresh logs to show the new entry
            loadData(currentPageRef.current, filterRef.current, accountFilterRef.current);
        } catch (e) {
            console.error('Failed to resend request', e);
        } finally {
            setResendingId(null);
        }
    };

    const executeClearLogs = async () => {
        setIsClearConfirmOpen(false);
        try {
            await invoke('clear_proxy_logs');
            setLogs([]);
            setStats({ total_requests: 0, success_count: 0, error_count: 0 });
            setTotalCount(0);
        } catch (e) {
            console.error("Failed to clear logs", e);
        }
    };

    const renderCollapsibleBody = (body?: string, title?: string) => {
        if (!body) return <span className="text-gray-400 italic">{t('monitor.details.payload_empty')}</span>;
        try {
            const obj = JSON.parse(body);
            return <JsonTreeView data={obj} title={title} />;
        } catch (e) {
            return <pre className="text-[10px] font-mono whitespace-pre-wrap text-gray-700 dark:text-gray-300">{body}</pre>;
        }
    };

    const getCopyPayload = (body: string) => {
        try {
            const obj = JSON.parse(body);
            return JSON.stringify(obj, null, 2);
        } catch (e) {
            return body;
        }
    };

    return (
        <div className={`flex flex-col bg-white dark:bg-base-100 rounded-xl shadow-sm border border-gray-100 dark:border-base-200 overflow-hidden ${className || 'flex-1'}`}>
            <div className="p-3 border-b border-gray-100 dark:border-base-200 space-y-3 bg-gray-50/30 dark:bg-base-200/30">
                <div className="flex items-center gap-4">
                    <button
                        onClick={toggleLogging}
                        className={`btn btn-sm gap-2 px-4 border font-bold ${isLoggingEnabled
                            ? 'bg-red-500 border-red-600 text-white animate-pulse'
                            : 'bg-white dark:bg-base-200 border-gray-300 text-gray-600'
                            }`}
                    >
                        <div className={`w-2.5 h-2.5 rounded-full ${isLoggingEnabled ? 'bg-white' : 'bg-gray-400'}`} />
                        {isLoggingEnabled ? t('monitor.logging_status.active') : t('monitor.logging_status.paused')}
                    </button>

                    <button
                        onClick={() => setLlmLogViewerOpen(true)}
                        className="btn btn-sm gap-1.5 px-3 border bg-white dark:bg-base-200 border-gray-300 text-gray-600 dark:text-gray-300"
                        title="查看 LLM 对话日志"
                    >
                        <MessageSquare size={14} />
                        LLM 日志
                    </button>

                    <div className="relative flex-1">
                        <Search className="absolute left-2.5 top-2 text-gray-400" size={14} />
                        <input
                            type="text"
                            placeholder={t('monitor.filters.placeholder')}
                            className="input input-sm input-bordered w-full pl-9 text-xs"
                            value={filter}
                            onChange={(e) => setFilter(e.target.value)}
                        />
                    </div>

                    <div className="relative">
                        <User className="absolute left-2.5 top-2 text-gray-400 z-10" size={14} />
                        <select
                            className="select select-sm select-bordered pl-8 text-xs min-w-[140px] max-w-[220px]"
                            value={accountFilter}
                            onChange={(e) => setAccountFilter(e.target.value)}
                            title={t('monitor.filters.by_account')}
                        >
                            <option value="">{t('monitor.filters.all_accounts')}</option>
                            {uniqueAccounts.map(email => (
                                <option key={email} value={email} title={email}>
                                    {email}
                                </option>
                            ))}
                        </select>
                    </div>

                    <div className="hidden lg:flex gap-4 text-[10px] font-bold uppercase">
                        <span className="text-blue-500">{formatCompactNumber(stats.total_requests)} {t('monitor.stats.total')}</span>
                        <span className="text-green-500">{formatCompactNumber(stats.success_count)} {t('monitor.stats.ok')}</span>
                        <span className="text-red-500">{formatCompactNumber(stats.error_count)} {t('monitor.stats.err')}</span>
                    </div>

                    <button onClick={() => loadData(currentPage, filter)} className="btn btn-sm btn-ghost text-gray-400" title={t('common.refresh')}>
                        <RefreshCw size={16} className={loading ? 'animate-spin' : ''} />
                    </button>
                    <button onClick={clearLogs} className="btn btn-sm btn-ghost text-gray-400">
                        <Trash2 size={16} />
                    </button>
                </div>

                <div className="flex flex-wrap items-center gap-2">
                    <span className="text-[10px] font-bold text-gray-400 uppercase">{t('monitor.filters.quick_filters')}</span>
                    {quickFilters.map(q => (
                        <button key={q.label} onClick={() => setFilter(q.value)} className={`px-2 py-0.5 rounded-full text-[10px] border ${filter === q.value ? 'bg-blue-500 text-white' : 'bg-white dark:bg-base-200 text-gray-500'}`}>
                            {q.label}
                        </button>
                    ))}
                    {(filter || accountFilter) && <button onClick={() => { setFilter(''); setAccountFilter(''); }} className="text-[10px] text-blue-500"> {t('monitor.filters.reset')} </button>}
                </div>
            </div>

            <LogTable
                logs={filteredLogs}
                loading={loading}
                inFlightTick={inFlightTick}
                onLogClick={async (log: ProxyRequestLog) => {
                    setLoadingDetail(true);
                    try {
                        const detail = await invoke<ProxyRequestLog>('get_proxy_log_detail', { logId: log.id });
                        setSelectedLog(detail);
                    } catch (e) {
                        console.error('Failed to load log detail', e);
                        setSelectedLog(log);
                    } finally {
                        setLoadingDetail(false);
                    }
                }}
                onResend={handleResend}
                t={t}
            />

            {/* Pagination Controls */}
            <div className="flex items-center justify-between px-4 py-3 bg-gray-50 dark:bg-base-200 border-t border-gray-200 dark:border-base-300 text-xs">
                <div className="flex items-center gap-2 whitespace-nowrap">
                    <span className="text-gray-500">{t('common.per_page')}</span>
                    <select
                        value={pageSize}
                        onChange={(e) => setPageSize(Number(e.target.value))}
                        className="select select-xs select-bordered w-16"
                    >
                        {PAGE_SIZE_OPTIONS.map(size => (
                            <option key={size} value={size}>{size}</option>
                        ))}
                    </select>
                </div>

                <div className="flex items-center gap-3">
                    <button
                        onClick={() => goToPage(currentPage - 1)}
                        disabled={currentPage <= 1 || loading}
                        className="btn btn-xs btn-ghost"
                    >
                        <ChevronLeft size={14} />
                    </button>
                    <span className="text-gray-600 dark:text-gray-400 min-w-[80px] text-center">
                        {currentPage} / {totalPages || 1}
                    </span>
                    <button
                        onClick={() => goToPage(currentPage + 1)}
                        disabled={currentPage >= totalPages || loading}
                        className="btn btn-xs btn-ghost"
                    >
                        <ChevronRight size={14} />
                    </button>
                </div>

                <div className="text-gray-500">
                    {t('common.pagination_info', { start: pageStart, end: pageEnd, total: totalCount })}
                </div>
            </div>

            {selectedLog && (
                <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/60 backdrop-blur-sm p-4" onClick={() => setSelectedLog(null)}>
                    <div className="bg-white dark:bg-base-100 rounded-xl shadow-2xl w-full max-w-4xl max-h-[90vh] flex flex-col overflow-hidden border border-gray-200 dark:border-base-300" onClick={e => e.stopPropagation()}>
                        {/* Modal Header */}
                        <div className="px-4 py-3 border-b border-gray-100 dark:border-base-300 flex items-center justify-between bg-gray-50 dark:bg-base-200">
                            <div className="flex items-center gap-3">
                                {loadingDetail && <div className="loading loading-spinner loading-sm"></div>}
                                <span className={`badge badge-sm text-white border-none ${selectedLog.status >= 200 && selectedLog.status < 400 ? 'badge-success' : 'badge-error'}`}>{selectedLog.status}</span>
                                <span className="font-mono font-bold text-gray-900 dark:text-base-content text-sm">{selectedLog.method}</span>
                                <span className="text-xs text-gray-500 dark:text-gray-400 font-mono truncate max-w-md hidden sm:inline">{selectedLog.url}</span>
                                <button
                                    type="button"
                                    className="btn btn-ghost btn-xs gap-1 ml-2 text-gray-400 hover:text-blue-500"
                                    title="重新发送此请求"
                                    onClick={async () => {
                                        setResendingId(selectedLog.id);
                                        try {
                                            const base = getProxyBaseUrl();
                                            const url = `${base}${selectedLog.url}`;
                                            const apiKey = sessionStorage.getItem('abv_admin_api_key');
                                            await fetch(url, {
                                                method: selectedLog.method,
                                                headers: {
                                                    'Content-Type': 'application/json',
                                                    ...(apiKey ? {
                                                        'Authorization': `Bearer ${apiKey}`,
                                                        'x-api-key': apiKey,
                                                    } : {}),
                                                },
                                                body: selectedLog.request_body,
                                            });
                                            loadData(currentPageRef.current, filterRef.current, accountFilterRef.current);
                                        } catch (e) {
                                            console.error('Failed to resend request', e);
                                        } finally {
                                            setResendingId(null);
                                        }
                                    }}
                                    disabled={resendingId === selectedLog.id || !selectedLog.request_body}
                                >
                                    <Repeat2 size={14} className={resendingId === selectedLog.id ? 'animate-spin' : ''} />
                                    <span className="text-[10px]">重发</span>
                                </button>
                            </div>
                            <button onClick={() => setSelectedLog(null)} className="btn btn-ghost btn-sm btn-circle text-gray-500 dark:text-gray-400 hover:dark:bg-base-300"><X size={18} /></button>
                        </div>

                        {/* Modal Content */}
                        <div className="flex-1 overflow-y-auto p-4 space-y-6 bg-white dark:bg-base-100">
                            {/* Metadata Section */}
                            <div className="bg-gray-50 dark:bg-base-200 p-5 rounded-xl border border-gray-200 dark:border-base-300 shadow-inner">
                                <div className="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 gap-y-5 gap-x-10">
                                    <div className="space-y-1.5">
                                        <span className="block text-gray-500 dark:text-gray-400 uppercase font-black text-[10px] tracking-widest">{t('monitor.details.time')}</span>
                                        <span className="font-mono font-semibold text-gray-900 dark:text-base-content text-xs">{new Date(selectedLog.timestamp).toLocaleString('en-GB', { year: 'numeric', month: '2-digit', day: '2-digit', hour: '2-digit', minute: '2-digit', second: '2-digit', hour12: false })}</span>
                                    </div>
                                    <div className="space-y-1.5">
                                        <span className="block text-gray-500 dark:text-gray-400 uppercase font-black text-[10px] tracking-widest">{t('monitor.details.duration')}</span>
                                        <span className="font-mono font-semibold text-gray-900 dark:text-base-content text-xs">{selectedLog.duration}ms</span>
                                    </div>
                                    <div className="space-y-1.5">
                                        <span className="block text-gray-500 dark:text-gray-400 uppercase font-black text-[10px] tracking-widest">{t('monitor.details.tokens')}</span>
                                        <div className="font-mono text-[11px] flex gap-2">
                                            <span className="text-blue-700 dark:text-blue-300 bg-blue-100 dark:bg-blue-900/40 px-2.5 py-1 rounded-md border border-blue-200 dark:border-blue-800/50 font-bold">In: {formatCompactNumber(selectedLog.input_tokens ?? 0)}</span>
                                            <span className="text-green-700 dark:text-green-300 bg-green-100 dark:bg-green-900/40 px-2.5 py-1 rounded-md border border-green-200 dark:border-green-800/50 font-bold">Out: {formatCompactNumber(selectedLog.output_tokens ?? 0)}</span>
                                        </div>
                                    </div>
                                </div>
                                <div className="mt-5 pt-5 border-t border-gray-200 dark:border-base-300">
                                    <div className="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-4 gap-4">
                                        {selectedLog.protocol && (
                                            <div className="space-y-1.5">
                                                <span className="block text-gray-500 dark:text-gray-400 uppercase font-black text-[10px] tracking-widest">请求协议</span>
                                                <span className={`inline-block px-2.5 py-1 rounded-md font-mono font-black text-xs uppercase ${selectedLog.protocol === 'openai' ? 'bg-emerald-100 text-emerald-700 dark:bg-emerald-900/40 dark:text-emerald-400 border border-emerald-200 dark:border-emerald-800/50' :
                                                    selectedLog.protocol === 'anthropic' ? 'bg-orange-100 text-orange-700 dark:bg-orange-900/40 dark:text-orange-400 border border-orange-200 dark:border-orange-800/50' :
                                                        selectedLog.protocol === 'gemini' ? 'bg-blue-100 text-blue-700 dark:bg-blue-900/40 dark:text-blue-400 border border-blue-200 dark:border-blue-800/50' :
                                                            'bg-gray-100 text-gray-700 dark:bg-gray-900/40 dark:text-gray-400'
                                                    }`}>
                                                    {selectedLog.protocol}
                                                </span>
                                            </div>
                                        )}
                                        <div className="space-y-1.5">
                                            <span className="block text-gray-500 dark:text-gray-400 uppercase font-black text-[10px] tracking-widest">模型</span>
                                            <span className="font-mono font-black text-blue-600 dark:text-blue-400 break-all text-sm">{selectedLog.model || '-'}</span>
                                        </div>
                                        {selectedLog.mapped_model && selectedLog.model !== selectedLog.mapped_model && (
                                            <div className="space-y-1.5">
                                                <span className="block text-gray-500 dark:text-gray-400 uppercase font-black text-[10px] tracking-widest">映射模型</span>
                                                <span className="font-mono font-black text-purple-600 dark:text-purple-400 break-all text-sm">{selectedLog.mapped_model}</span>
                                            </div>
                                        )}
                                        {selectedLog.upstream_protocol && (
                                            <div className="space-y-1.5">
                                                <span className="block text-gray-500 dark:text-gray-400 uppercase font-black text-[10px] tracking-widest">供应商协议</span>
                                                <span className={`inline-block px-2.5 py-1 rounded-md font-mono font-black text-xs uppercase ${selectedLog.upstream_protocol === 'openai' ? 'bg-emerald-100 text-emerald-700 dark:bg-emerald-900/40 dark:text-emerald-400 border border-emerald-200 dark:border-emerald-800/50' :
                                                    selectedLog.upstream_protocol === 'anthropic' ? 'bg-orange-100 text-orange-700 dark:bg-orange-900/40 dark:text-orange-400 border border-orange-200 dark:border-orange-800/50' :
                                                        selectedLog.upstream_protocol === 'gemini' ? 'bg-blue-100 text-blue-700 dark:bg-blue-900/40 dark:text-blue-400 border border-blue-200 dark:border-blue-800/50' :
                                                            'bg-gray-100 text-gray-700 dark:bg-gray-900/40 dark:text-gray-400'
                                                    }`}>
                                                    {selectedLog.upstream_protocol}
                                                </span>
                                            </div>
                                        )}
                                    </div>
                                    {(selectedLog.upstream_model || selectedLog.upstream_url) && (
                                        <div className="grid grid-cols-1 sm:grid-cols-2 gap-4 mt-4">
                                            {selectedLog.upstream_model && (
                                                <div className="space-y-1.5">
                                                    <span className="block text-gray-500 dark:text-gray-400 uppercase font-black text-[10px] tracking-widest">上游模型</span>
                                                    <span className="font-mono font-black text-emerald-600 dark:text-emerald-400 break-all text-sm">{selectedLog.upstream_model}</span>
                                                </div>
                                            )}
                                            {selectedLog.upstream_url && (
                                                <div className="space-y-1.5">
                                                    <span className="block text-gray-500 dark:text-gray-400 uppercase font-black text-[10px] tracking-widest">上游 URL</span>
                                                    <span className="font-mono font-semibold text-gray-700 dark:text-gray-300 break-all text-xs">{selectedLog.upstream_url}</span>
                                                </div>
                                            )}
                                        </div>
                                    )}
                                </div>
                                {(selectedLog.provider_name || selectedLog.account_email) && (
                                    <div className="mt-5 pt-5 border-t border-gray-200 dark:border-base-300">
                                        <span className="block text-gray-500 dark:text-gray-400 uppercase font-black text-[10px] tracking-widest mb-2">{selectedLog.provider_name ? t('monitor.details.provider_used') : t('monitor.details.account_used')}</span>
                                        {selectedLog.provider_name ? (
                                            <span className="px-2 py-1 rounded bg-blue-100 dark:bg-blue-900/40 text-blue-700 dark:text-blue-300 text-xs font-medium">
                                                {selectedLog.provider_name}
                                            </span>
                                        ) : (
                                            <span className="font-mono font-semibold text-gray-900 dark:text-base-content text-xs">{selectedLog.account_email}</span>
                                        )}
                                    </div>
                                )}
                            </div>

                            {/* Payloads */}
                            <div className="space-y-4">
                                {/* Tab switching: 原始报文 / 供应商报文 / 可视化 */}
                                <div className="flex items-center gap-1 bg-gray-100 dark:bg-base-200 p-1 rounded-lg w-fit">
                                    <button
                                        type="button"
                                        onClick={() => setDetailViewMode('raw')}
                                        className={`px-3 py-1.5 text-xs font-bold rounded-md transition-all ${detailViewMode === 'raw' ? 'bg-white dark:bg-base-100 shadow-sm text-blue-600 dark:text-blue-400' : 'text-gray-500 hover:text-gray-700 dark:hover:text-gray-300'}`}
                                    >
                                        原始报文
                                    </button>
                                    <button
                                        type="button"
                                        onClick={() => setDetailViewMode('upstream')}
                                        className={`px-3 py-1.5 text-xs font-bold rounded-md transition-all flex items-center gap-1 ${detailViewMode === 'upstream' ? 'bg-white dark:bg-base-100 shadow-sm text-green-600 dark:text-green-400' : 'text-gray-500 hover:text-gray-700 dark:hover:text-gray-300'}`}
                                    >
                                        <Server size={12} />
                                        供应商报文
                                    </button>
                                    <button
                                        type="button"
                                        onClick={() => setDetailViewMode('compare')}
                                        className={`px-3 py-1.5 text-xs font-bold rounded-md transition-all flex items-center gap-1 ${detailViewMode === 'compare' ? 'bg-white dark:bg-base-100 shadow-sm text-orange-600 dark:text-orange-400' : 'text-gray-500 hover:text-gray-700 dark:hover:text-gray-300'}`}
                                    >
                                        <Repeat2 size={12} />
                                        报文对比
                                    </button>
                                    <button
                                        type="button"
                                        onClick={() => setDetailViewMode('visual')}
                                        className={`px-3 py-1.5 text-xs font-bold rounded-md transition-all flex items-center gap-1.5 ${detailViewMode === 'visual' ? 'bg-white dark:bg-base-100 shadow-sm text-blue-600 dark:text-blue-400' : 'text-gray-500 hover:text-gray-700 dark:hover:text-gray-300'}`}
                                    >
                                        <Eye size={12} />
                                        可视化
                                    </button>
                                </div>

                                {/* Tab 1: 原始报文 (8150 交互) */}
                                {detailViewMode === 'raw' && (
                                    <div className="space-y-4">
                                        <div>
                                            <div className="flex items-center justify-between mb-2">
                                                <h3 className="text-xs font-bold uppercase text-gray-400 flex items-center gap-2">{t('monitor.details.request_payload')}</h3>
                                                <button
                                                    type="button"
                                                    className="btn btn-ghost btn-xs gap-1"
                                                    onClick={async () => {
                                                        if (!selectedLog.request_body) return;
                                                        const success = await copyToClipboard(getCopyPayload(selectedLog.request_body));
                                                        if (success) {
                                                            setCopiedRequestId(selectedLog.id);
                                                            setTimeout(() => {
                                                                setCopiedRequestId((current) => (current === selectedLog.id ? null : current));
                                                            }, 2000);
                                                        }
                                                    }}
                                                    disabled={!selectedLog.request_body}
                                                    title={copiedRequestId === selectedLog.id ? t('proxy.config.btn_copied') : t('proxy.config.btn_copy')}
                                                    aria-label={t('proxy.config.btn_copy')}
                                                >
                                                    {copiedRequestId === selectedLog.id ? (
                                                        <CheckCircle size={12} className="text-green-500" />
                                                    ) : (
                                                        <Copy size={12} />
                                                    )}
                                                    <span className="text-[10px]">
                                                        {copiedRequestId === selectedLog.id ? t('proxy.config.btn_copied') : t('proxy.config.btn_copy')}
                                                    </span>
                                                </button>
                                            </div>
                                            <div className="bg-gray-50 dark:bg-base-300 rounded-lg p-3 border border-gray-100 dark:border-base-300 overflow-hidden">{renderCollapsibleBody(selectedLog.request_body, t('monitor.details.request_payload'))}</div>
                                        </div>
                                        <div>
                                            <div className="flex items-center justify-between mb-2">
                                                <h3 className="text-xs font-bold uppercase text-gray-400 flex items-center gap-2">{t('monitor.details.response_payload')}</h3>
                                                <button
                                                    type="button"
                                                    className="btn btn-ghost btn-xs gap-1"
                                                    onClick={async () => {
                                                        if (!selectedLog.response_body) return;
                                                        const success = await copyToClipboard(getCopyPayload(selectedLog.response_body));
                                                        if (success) {
                                                            setCopiedRequestId(selectedLog.id ? `${selectedLog.id}-response` : null);
                                                            setTimeout(() => {
                                                                setCopiedRequestId((current) =>
                                                                    current === `${selectedLog.id}-response` ? null : current
                                                                );
                                                            }, 2000);
                                                        }
                                                    }}
                                                    disabled={!selectedLog.response_body}
                                                    title={copiedRequestId === `${selectedLog.id}-response` ? t('proxy.config.btn_copied') : t('proxy.config.btn_copy')}
                                                    aria-label={t('proxy.config.btn_copy')}
                                                >
                                                    {copiedRequestId === `${selectedLog.id}-response` ? (
                                                        <CheckCircle size={12} className="text-green-500" />
                                                    ) : (
                                                        <Copy size={12} />
                                                    )}
                                                    <span className="text-[10px]">
                                                        {copiedRequestId === `${selectedLog.id}-response` ? t('proxy.config.btn_copied') : t('proxy.config.btn_copy')}
                                                    </span>
                                                </button>
                                            </div>
                                            <div className="bg-gray-50 dark:bg-base-300 rounded-lg p-3 border border-gray-100 dark:border-base-300 overflow-hidden">{renderCollapsibleBody(selectedLog.response_body, t('monitor.details.response_payload'))}</div>
                                        </div>
                                    </div>
                                )}

                                {/* Tab 2: 供应商报文 (provider 交互) */}
                                {detailViewMode === 'upstream' && (
                                    <div className="space-y-4">
                                        {selectedLog.upstream_request_body ? (
                                            <div>
                                                <div className="flex items-center justify-between mb-2">
                                                    <h3 className="text-xs font-bold uppercase text-gray-400 flex items-center gap-2">
                                                        <ArrowRight size={12} className="text-green-500" />
                                                        请求（发给供应商）
                                                    </h3>
                                                    <button
                                                        type="button"
                                                        className="btn btn-ghost btn-xs gap-1"
                                                        onClick={async () => {
                                                            if (!selectedLog.upstream_request_body) return;
                                                            const success = await copyToClipboard(getCopyPayload(selectedLog.upstream_request_body!));
                                                            if (success) {
                                                                setCopiedRequestId(selectedLog.id ? `${selectedLog.id}-upstream-req` : null);
                                                                setTimeout(() => {
                                                                    setCopiedRequestId((current) =>
                                                                        current === `${selectedLog.id}-upstream-req` ? null : current
                                                                    );
                                                                }, 2000);
                                                            }
                                                        }}
                                                        title="复制请求"
                                                        aria-label="复制请求"
                                                    >
                                                        {copiedRequestId === `${selectedLog.id}-upstream-req` ? (
                                                            <CheckCircle size={12} className="text-green-500" />
                                                        ) : (
                                                            <Copy size={12} />
                                                        )}
                                                        <span className="text-[10px]">复制</span>
                                                    </button>
                                                </div>
                                                <div className="bg-gray-50 dark:bg-base-300 rounded-lg p-3 border border-green-100 dark:border-green-900/30 overflow-hidden">{renderCollapsibleBody(selectedLog.upstream_request_body, '请求（发给供应商）')}</div>
                                            </div>
                                        ) : (
                                            <div className="flex items-center gap-2 py-4 text-gray-400 dark:text-gray-500">
                                                <Server size={14} />
                                                <span className="text-xs italic">无供应商交互记录（非供应商通道或未启用报文记录）</span>
                                            </div>
                                        )}
                                        {selectedLog.upstream_response_body && (
                                            <div>
                                                <div className="flex items-center justify-between mb-2">
                                                    <h3 className="text-xs font-bold uppercase text-gray-400 flex items-center gap-2">
                                                        <ArrowRight size={12} className="text-green-500 rotate-180" />
                                                        响应（来自供应商）
                                                    </h3>
                                                    <button
                                                        type="button"
                                                        className="btn btn-ghost btn-xs gap-1"
                                                        onClick={async () => {
                                                            if (!selectedLog.upstream_response_body) return;
                                                            const success = await copyToClipboard(getCopyPayload(selectedLog.upstream_response_body!));
                                                            if (success) {
                                                                setCopiedRequestId(selectedLog.id ? `${selectedLog.id}-upstream-resp` : null);
                                                                setTimeout(() => {
                                                                    setCopiedRequestId((current) =>
                                                                        current === `${selectedLog.id}-upstream-resp` ? null : current
                                                                    );
                                                                }, 2000);
                                                            }
                                                        }}
                                                        title="复制响应"
                                                        aria-label="复制响应"
                                                    >
                                                        {copiedRequestId === `${selectedLog.id}-upstream-resp` ? (
                                                            <CheckCircle size={12} className="text-green-500" />
                                                        ) : (
                                                            <Copy size={12} />
                                                        )}
                                                        <span className="text-[10px]">复制</span>
                                                    </button>
                                                </div>
                                                <div className="bg-gray-50 dark:bg-base-300 rounded-lg p-3 border border-green-100 dark:border-green-900/30 overflow-hidden">{renderCollapsibleBody(selectedLog.upstream_response_body, '响应（来自供应商）')}</div>
                                            </div>
                                        )}
                                    </div>
                                )}

                                {/* Tab 3: 报文对比 (diff) */}
                                {detailViewMode === 'compare' && (
                                    <div className="space-y-4">
                                        {/* Request diff */}
                                        <div>
                                            <DiffView
                                                left={selectedLog.request_body ?? ''}
                                                right={selectedLog.upstream_request_body ?? ''}
                                                leftLabel="原始请求（客户端 → 8150）"
                                                rightLabel="供应商请求（8150 → 供应商）"
                                            />
                                        </div>
                                        {/* Response diff */}
                                        <div>
                                            <DiffView
                                                left={selectedLog.response_body ?? ''}
                                                right={selectedLog.upstream_response_body ?? ''}
                                                leftLabel="原始响应（8150 → 客户端）"
                                                rightLabel="供应商响应（供应商 → 8150）"
                                            />
                                        </div>
                                    </div>
                                )}

                                {/* Tab 4: 可视化 (8150 原始) */}
                                {detailViewMode === 'visual' && (
                                    <VisualView log={selectedLog} />
                                )}
                            </div>
                        </div>
                    </div>
                </div>
            )}

            <ModalDialog
                isOpen={isClearConfirmOpen}
                title={t('monitor.dialog.clear_title')}
                message={t('monitor.dialog.clear_msg')}
                type="confirm"
                confirmText={t('common.delete')}
                isDestructive={true}
                onConfirm={executeClearLogs}
                onCancel={() => setIsClearConfirmOpen(false)}
            />

            <LlmLogViewer isOpen={llmLogViewerOpen} onClose={() => setLlmLogViewerOpen(false)} />
        </div>
    );
};
