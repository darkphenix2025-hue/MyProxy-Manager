import React, { useEffect, useState, useRef, useMemo } from 'react';
import { listen } from '@tauri-apps/api/event';
import ModalDialog from '../common/ModalDialog';
import { useTranslation } from 'react-i18next';
import { request as invoke } from '../../utils/request';
import { Trash2, Search, X, Copy, CheckCircle, ChevronLeft, ChevronRight, RefreshCw, User, ArrowRight, MessageSquare, Server, Cpu, AlertTriangle, ChevronDown, ChevronUp, Eye } from 'lucide-react';

import { AppConfig } from '../../types/config';
import { formatCompactNumber } from '../../utils/format';
import { useAccountStore } from '../../stores/useAccountStore';
import { isTauri } from '../../utils/env';
import { copyToClipboard } from '../../utils/clipboard';


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
    t: any;
}

const LogTable: React.FC<LogTableProps> = ({
    logs,
    loading,
    onLogClick,
    t
}) => {
    return (
        <div
            className="flex-1 overflow-y-auto overflow-x-auto bg-white dark:bg-base-100"
        >
            <table className="table table-xs w-full">
                <thead className="bg-gray-50 dark:bg-base-200 text-gray-500 sticky top-0 z-10">
                    <tr>
                        <th style={{ width: '60px' }}>{t('monitor.table.status')}</th>
                        <th style={{ width: '60px' }}>{t('monitor.table.method')}</th>
                        <th style={{ width: '220px' }}>{t('monitor.table.model')}</th>
                        <th style={{ width: '70px' }}>{t('monitor.table.protocol')}</th>
                        <th style={{ width: '140px' }}>{t('monitor.table.account')}</th>
                        <th style={{ width: '180px' }}>{t('monitor.table.path')}</th>
                        <th className="text-right" style={{ width: '90px' }}>{t('monitor.table.usage')}</th>
                        <th className="text-right" style={{ width: '80px' }}>{t('monitor.table.duration')}</th>
                        <th className="text-right" style={{ width: '80px' }}>{t('monitor.table.time')}</th>
                    </tr>
                </thead>
                <tbody className="font-mono text-gray-700 dark:text-gray-300">
                    {logs.map((log) => (
                        <tr
                            key={log.id}
                            className="hover:bg-blue-50 dark:hover:bg-blue-900/20 cursor-pointer"
                            onClick={() => onLogClick(log)}
                        >
                            <td style={{ width: '60px' }}>
                                <span className={`badge badge-xs text-white border-none ${log.status >= 200 && log.status < 400 ? 'badge-success' : 'badge-error'}`}>
                                    {log.status}
                                </span>
                            </td>
                            <td className="font-bold" style={{ width: '60px' }}>{log.method}</td>
                            <td className="text-blue-600 truncate" style={{ width: '220px', maxWidth: '220px' }}>
                                {log.mapped_model && log.model !== log.mapped_model
                                    ? `${log.model} => ${log.mapped_model}`
                                    : (log.model || '-')}
                            </td>
                            <td style={{ width: '70px' }}>
                                {log.protocol && (
                                    <span className={`badge badge-xs text-white border-none ${log.protocol === 'openai' ? 'bg-green-500' :
                                        log.protocol === 'anthropic' ? 'bg-orange-500' :
                                            log.protocol === 'gemini' ? 'bg-blue-500' : 'bg-gray-400'
                                        }`}>
                                        {log.protocol === 'openai' ? 'OpenAI' :
                                            log.protocol === 'anthropic' ? 'Claude' :
                                                log.protocol === 'gemini' ? 'Gemini' : log.protocol}
                                    </span>
                                )}
                            </td>
                            <td className="text-gray-600 dark:text-gray-400 truncate text-[10px]" style={{ width: '140px', maxWidth: '140px' }} title={log.provider_name || log.account_email || ''}>
                                {log.provider_name ? (
                                    <span className="inline-flex items-center gap-1">
                                        <span className="px-1 py-0.5 rounded bg-blue-100 dark:bg-blue-900/40 text-blue-700 dark:text-blue-300 text-[10px] font-medium">
                                            {log.provider_name}
                                        </span>
                                    </span>
                                ) : log.account_email ? (
                                    log.account_email.replace(/(.{3}).*(@.*)/, '$1***$2')
                                ) : '-'}
                            </td>
                            <td className="truncate" style={{ width: '180px', maxWidth: '180px' }}>{log.url}</td>
                            <td className="text-right text-[9px]" style={{ width: '90px' }}>
                                {log.input_tokens != null && <div>I: {formatCompactNumber(log.input_tokens)}</div>}
                                {log.output_tokens != null && <div>O: {formatCompactNumber(log.output_tokens)}</div>}
                            </td>
                            <td className="text-right" style={{ width: '80px' }}>{log.duration}ms</td>
                            <td className="text-right text-[10px]" style={{ width: '80px' }}>
                                {new Date(log.timestamp).toLocaleTimeString()}
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

    const truncateText = (text: string, maxLen = 120) => {
        if (!text) return '';
        return text.length > maxLen ? text.slice(0, maxLen) + '...' : text;
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

    const SectionCard: React.FC<{
        icon: React.ReactNode;
        title: string;
        sectionKey: string;
        sizeBytes?: number;
        children: React.ReactNode;
    }> = ({ icon, title, sizeBytes, sectionKey, children }) => (
        <div className="border border-gray-100 dark:border-base-300 rounded-lg overflow-hidden">
            <button
                className="w-full flex items-center gap-2 px-3 py-2 bg-gray-50 dark:bg-base-200 hover:bg-gray-100 dark:hover:bg-base-300 transition-colors"
                onClick={() => toggleSection(sectionKey)}
            >
                {icon}
                <span className="text-xs font-bold text-gray-700 dark:text-gray-300">{title}</span>
                {sizeBytes != null && sizeBytes > 0 && (
                    <span className="text-[9px] font-mono text-gray-400 dark:text-gray-500 bg-gray-100 dark:bg-base-300 px-1.5 py-0.5 rounded">{formatBytes(sizeBytes)}</span>
                )}
                <span className="ml-auto">
                    {expandedSections[sectionKey] ? <ChevronUp size={14} className="text-gray-400" /> : <ChevronDown size={14} className="text-gray-400" />}
                </span>
            </button>
            {expandedSections[sectionKey] && (
                <div className="p-3 bg-white dark:bg-base-100">{children}</div>
            )}
        </div>
    );

    // Parse request messages
    const renderRequestMessages = () => {
        if (protocol === 'openai' && requestObj?.messages) {
            return (requestObj.messages as any[]).map((msg: any, i: number) => (
                <div key={i} className="mb-2 last:mb-0">
                    <div className="flex items-start gap-2">
                        <span className={`px-1.5 py-0.5 rounded text-[9px] font-bold shrink-0 ${
                            msg.role === 'user' ? 'bg-blue-100 text-blue-700 dark:bg-blue-900/40 dark:text-blue-300' :
                            msg.role === 'assistant' ? 'bg-green-100 text-green-700 dark:bg-green-900/40 dark:text-green-300' :
                            msg.role === 'system' ? 'bg-purple-100 text-purple-700 dark:bg-purple-900/40 dark:text-purple-300' :
                            'bg-gray-100 text-gray-600 dark:bg-gray-800 dark:text-gray-400'
                        }`}>
                            {msg.role}
                        </span>
                        <div className="flex-1 min-w-0">
                            {typeof msg.content === 'string' ? (
                                <p className="text-[10px] font-mono text-gray-600 dark:text-gray-400 break-all whitespace-pre-wrap">{truncateText(msg.content, 300)}</p>
                            ) : Array.isArray(msg.content) ? (
                                <div className="space-y-1">
                                    {msg.content.map((part: any, j: number) => (
                                        <div key={j} className="text-[10px] font-mono text-gray-600 dark:text-gray-400">
                                            {part.type === 'text' && <p className="break-all whitespace-pre-wrap">{truncateText(part.text, 300)}</p>}
                                            {part.type === 'image_url' && <span className="text-amber-600 dark:text-amber-400">[Image]</span>}
                                            {part.type === 'tool_use' && <span className="text-purple-600 dark:text-purple-400">[Tool: {part.name}]</span>}
                                            {part.type === 'tool_result' && <span className="text-cyan-600 dark:text-cyan-400">[Tool Result]</span>}
                                        </div>
                                    ))}
                                </div>
                            ) : null}
                        </div>
                    </div>
                </div>
            ));
        }

        if ((protocol === 'anthropic') && requestObj?.messages) {
            return (requestObj.messages as any[]).map((msg: any, i: number) => (
                <div key={i} className="mb-2 last:mb-0">
                    <div className="flex items-start gap-2">
                        <span className={`px-1.5 py-0.5 rounded text-[9px] font-bold shrink-0 ${
                            msg.role === 'user' ? 'bg-blue-100 text-blue-700 dark:bg-blue-900/40 dark:text-blue-300' :
                            msg.role === 'assistant' ? 'bg-green-100 text-green-700 dark:bg-green-900/40 dark:text-green-300' :
                            'bg-gray-100 text-gray-600 dark:bg-gray-800 dark:text-gray-400'
                        }`}>
                            {msg.role}
                        </span>
                        <div className="flex-1 min-w-0">
                            {Array.isArray(msg.content) ? (
                                <div className="space-y-1">
                                    {msg.content.map((part: any, j: number) => (
                                        <div key={j} className="text-[10px] font-mono text-gray-600 dark:text-gray-400">
                                            {part.type === 'text' && <p className="break-all whitespace-pre-wrap">{truncateText(part.text, 300)}</p>}
                                            {part.type === 'image' && <span className="text-amber-600 dark:text-amber-400">[Image: {part.source?.type}]</span>}
                                            {part.type === 'tool_use' && <span className="text-purple-600 dark:text-purple-400">[Tool: {part.name}]</span>}
                                            {part.type === 'tool_result' && <span className="text-cyan-600 dark:text-cyan-400">[Tool Result]</span>}
                                        </div>
                                    ))}
                                </div>
                            ) : typeof msg.content === 'string' ? (
                                <p className="text-[10px] font-mono text-gray-600 dark:text-gray-400 break-all whitespace-pre-wrap">{truncateText(msg.content, 300)}</p>
                            ) : null}
                        </div>
                    </div>
                </div>
            ));
        }

        return <p className="text-[10px] text-gray-400 italic">No messages found</p>;
    };

    const renderRequestSystem = () => {
        if (protocol === 'openai') {
            const sysMsg = requestObj?.messages?.find((m: any) => m.role === 'system');
            if (!sysMsg) return <p className="text-[10px] text-gray-400 italic">No system prompt</p>;
            return <pre className="text-[10px] font-mono text-gray-600 dark:text-gray-400 whitespace-pre-wrap break-all">{truncateText(typeof sysMsg.content === 'string' ? sysMsg.content : JSON.stringify(sysMsg.content), 500)}</pre>;
        }
        const sys = requestObj?.system;
        if (!sys) return <p className="text-[10px] text-gray-400 italic">No system prompt</p>;
        if (typeof sys === 'string') return <pre className="text-[10px] font-mono text-gray-600 dark:text-gray-400 whitespace-pre-wrap break-all">{truncateText(sys, 500)}</pre>;
        if (Array.isArray(sys)) return sys.map((s: any, i: number) => (
            <pre key={i} className="text-[10px] font-mono text-gray-600 dark:text-gray-400 whitespace-pre-wrap break-all mb-1 last:mb-0">{truncateText(s.text || JSON.stringify(s), 500)}</pre>
        ));
        return <pre className="text-[10px] font-mono text-gray-600 dark:text-gray-400 whitespace-pre-wrap break-all">{truncateText(JSON.stringify(sys), 500)}</pre>;
    };

    const renderRequestTools = () => {
        const tools = requestObj?.tools;
        if (!tools || !Array.isArray(tools) || tools.length === 0) return <p className="text-[10px] text-gray-400 italic">No tools</p>;
        return (
            <div className="space-y-1">
                {tools.map((tool: any, i: number) => (
                    <div key={i} className="text-[10px]">
                        <span className="font-mono font-bold text-purple-600 dark:text-purple-400">{tool.name || (tool.function?.name)}</span>
                        {tool.description && <span className="text-gray-500 dark:text-gray-400 ml-1">{truncateText(tool.description, 100)}</span>}
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
                        <pre className="text-[10px] font-mono text-gray-700 dark:text-gray-300 whitespace-pre-wrap break-all">{truncateText(choice.message.content, 500)}</pre>
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
                        <pre className="text-[10px] font-mono text-gray-700 dark:text-gray-300 whitespace-pre-wrap break-all">{truncateText(responseObj.content, 500)}</pre>
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
                        <pre className="text-[10px] font-mono text-gray-700 dark:text-gray-300 whitespace-pre-wrap break-all">{truncateText(block.text, 500)}</pre>
                    )}
                    {block.type === 'tool_use' && (
                        <pre className="text-[10px] font-mono text-purple-600 dark:text-purple-400 whitespace-pre-wrap">{truncateText(JSON.stringify(block.input), 300)}</pre>
                    )}
                    {block.type === 'thinking' && (
                        <details className="text-[10px]">
                            <summary className="cursor-pointer text-amber-600 dark:text-amber-400 font-bold">[Thinking...]</summary>
                            <pre className="text-[10px] font-mono text-gray-600 dark:text-gray-400 whitespace-pre-wrap break-all mt-1">{truncateText(block.thinking, 500)}</pre>
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
                            {part.text && <pre className="text-[10px] font-mono text-gray-700 dark:text-gray-300 whitespace-pre-wrap break-all">{truncateText(part.text, 500)}</pre>}
                            {part.functionCall && <div className="text-[10px] text-purple-600 dark:text-purple-400">[Function: {part.functionCall.name}]</div>}
                        </div>
                    ))}
                </div>
            ));
        }

        // Fallback
        return <pre className="text-[10px] font-mono text-gray-600 dark:text-gray-400 whitespace-pre-wrap break-all">{truncateText(JSON.stringify(responseObj, null, 2), 500)}</pre>;
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
                            <div className="text-[9px] text-gray-400">port 8045</div>
                        </div>
                    </div>
                    <ArrowRight size={14} className="text-gray-300 dark:text-gray-600 rotate-90 sm:rotate-0" />
                    <div className="flex items-center gap-2 bg-white dark:bg-base-100 px-3 py-2 rounded-lg shadow-sm border border-gray-100 dark:border-base-300">
                        <Cpu size={14} className="text-green-500" />
                        <div>
                            <div className="text-[10px] font-bold text-gray-700 dark:text-gray-300 truncate max-w-[140px]">{upstreamName}</div>
                            <div className="text-[9px] text-gray-400" title={modelMap}>{truncateText(modelMap, 20)}</div>
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
    const accountFilterRef = useRef(accountFilter);
    const currentPageRef = useRef(1);
    const [selectedLog, setSelectedLog] = useState<ProxyRequestLog | null>(null);
    const [detailViewMode, setDetailViewMode] = useState<'raw' | 'visual'>('raw');

    // Reset view mode when dialog opens for a different log
    useEffect(() => {
        setDetailViewMode('raw');
    }, [selectedLog?.id]);
    const [isLoggingEnabled, setIsLoggingEnabled] = useState(false);
    const [isClearConfirmOpen, setIsClearConfirmOpen] = useState(false);
    const [copiedRequestId, setCopiedRequestId] = useState<string | null>(null);

    const { accounts, fetchAccounts } = useAccountStore();

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

    useEffect(() => {
        isMountedRef.current = true;
        loadData();
        fetchAccounts();

        let unlistenFn: (() => void) | null = null;
        let updateTimeout: number | null = null;

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

                // Check if this log already exists (deduplicate at event level)
                const alreadyExists = pendingLogsRef.current.some(log => log.id === newLog.id);
                if (alreadyExists) {
                    console.debug('[ProxyMonitor] Duplicate event ignored:', newLog.id);
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
                            // Deduplicate by id
                            const existingIds = new Set(prev.map(log => log.id));
                            const uniqueNewLogs = currentPending.filter(log => !existingIds.has(log.id));
                            // Merge and sort by timestamp descending (newest first)
                            const merged = [...uniqueNewLogs, ...prev];
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

    const formatBody = (body?: string) => {
        if (!body) return <span className="text-gray-400 italic">{t('monitor.details.payload_empty')}</span>;
        try {
            const obj = JSON.parse(body);
            return <pre className="text-[10px] font-mono whitespace-pre-wrap text-gray-700 dark:text-gray-300">{JSON.stringify(obj, null, 2)}</pre>;
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
                                        <span className="font-mono font-semibold text-gray-900 dark:text-base-content text-xs">{new Date(selectedLog.timestamp).toLocaleString()}</span>
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
                                    <div className="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 gap-5">
                                        {selectedLog.protocol && (
                                            <div className="space-y-1.5">
                                                <span className="block text-gray-500 dark:text-gray-400 uppercase font-black text-[10px] tracking-widest">{t('monitor.details.protocol')}</span>
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
                                            <span className="block text-gray-500 dark:text-gray-400 uppercase font-black text-[10px] tracking-widest">{t('monitor.details.model')}</span>
                                            <span className="font-mono font-black text-blue-600 dark:text-blue-400 break-all text-sm">{selectedLog.model || '-'}</span>
                                        </div>
                                        {selectedLog.mapped_model && selectedLog.model !== selectedLog.mapped_model && (
                                            <div className="space-y-1.5">
                                                <span className="block text-gray-500 dark:text-gray-400 uppercase font-black text-[10px] tracking-widest">{t('monitor.details.mapped_model')}</span>
                                                <span className="font-mono font-black text-green-600 dark:text-green-400 break-all text-sm">{selectedLog.mapped_model}</span>
                                            </div>
                                        )}
                                    </div>
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
                                {/* Tab switching */}
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
                                        onClick={() => setDetailViewMode('visual')}
                                        className={`px-3 py-1.5 text-xs font-bold rounded-md transition-all flex items-center gap-1.5 ${detailViewMode === 'visual' ? 'bg-white dark:bg-base-100 shadow-sm text-blue-600 dark:text-blue-400' : 'text-gray-500 hover:text-gray-700 dark:hover:text-gray-300'}`}
                                    >
                                        <Eye size={12} />
                                        可视化
                                    </button>
                                </div>

                                {detailViewMode === 'raw' ? (
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
                                            <div className="bg-gray-50 dark:bg-base-300 rounded-lg p-3 border border-gray-100 dark:border-base-300 overflow-hidden">{formatBody(selectedLog.request_body)}</div>
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
                                            <div className="bg-gray-50 dark:bg-base-300 rounded-lg p-3 border border-gray-100 dark:border-base-300 overflow-hidden">{formatBody(selectedLog.response_body)}</div>
                                        </div>
                                    </div>
                                ) : (
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
        </div>
    );
};
