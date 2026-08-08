import { useState, useEffect, useCallback } from "react";
import { request as invoke } from "../../utils/request";

// --- Types ---

interface TraceSummary {
  trace_id: string;
  timestamp: string;
  model: string | null;
  stages: string[];
}

interface TraceDetail {
  trace_id: string;
  client_request: Record<string, unknown> | null;
  upstream_request: Record<string, unknown> | null;
  upstream_response: Record<string, unknown> | null;
  client_response: Record<string, unknown> | null;
}

interface ChatMessage {
  role: "user" | "assistant" | "thinking";
  label: string;
  content: string;
}

// --- Helpers ---

function extractTextContent(content: unknown): string {
  if (typeof content === "string") return content;
  if (Array.isArray(content)) {
    return content
      .filter((p: any) => p.type === "text")
      .map((p: any) => p.text)
      .join("\n");
  }
  return typeof content === "object" && content !== null
    ? JSON.stringify(content, null, 2)
    : String(content);
}

function extractThinkingFromMessages(content: unknown): string {
  if (!Array.isArray(content)) return "";
  const parts: string[] = [];
  for (const p of content as any[]) {
    if (p.type === "thinking" && p.thinking) {
      parts.push(p.thinking);
    }
  }
  return parts.join("\n");
}

function buildChatMessages(detail: TraceDetail): ChatMessage[] {
  const messages: ChatMessage[] = [];

  // Extract conversation history from client_request
  const cr = detail.client_request;
  if (cr?.body) {
    const body = cr.body as Record<string, unknown>;
    const msgArray = body.messages as
      | Array<{
          role: string;
          content: unknown;
        }>
      | undefined;
    if (Array.isArray(msgArray)) {
      for (const msg of msgArray) {
        if (msg.role === "user") {
          const text = extractTextContent(msg.content);
          if (text) {
            messages.push({ role: "user", label: "You", content: text });
          }
        } else if (msg.role === "assistant") {
          // Extract thinking from assistant history messages
          const thinking = extractThinkingFromMessages(msg.content);
          if (thinking) {
            messages.push({
              role: "thinking",
              label: "思考",
              content: thinking,
            });
          }
          const text = extractTextContent(msg.content);
          if (text) {
            messages.push({
              role: "assistant",
              label: "Assistant",
              content: text,
            });
          }
        }
      }
    }
  }

  // Extract thinking from client_response
  const cresp = detail.client_response;
  if (cresp?.body) {
    const body = cresp.body as Record<string, unknown>;
    if (typeof body.thinking === "string" && body.thinking) {
      messages.push({
        role: "thinking",
        label: "思考",
        content: body.thinking,
      });
    }
    if (typeof body.content === "string" && body.content) {
      messages.push({
        role: "assistant",
        label: "Assistant",
        content: body.content,
      });
    }
  }

  return messages;
}

const TRUNCATE_THRESHOLD = 2000;
const TRUNCATE_PREVIEW = 500;

// Module-level cache: persists expanded state even when components unmount/remount
const expandedCache = new Map<string, boolean>();

function TruncatableText({
  text,
  className,
}: {
  text: string;
  className: string;
}) {
  const needsTruncation = text.length > TRUNCATE_THRESHOLD;
  // Use content hash as cache key so state persists across unmount/remount
  const cacheKey = text.length > 100 ? text.slice(0, 100) : text;
  const [expanded, setExpanded] = useState(() => expandedCache.get(cacheKey) ?? false);

  const display =
    expanded || !needsTruncation
      ? text
      : text.slice(0, TRUNCATE_PREVIEW) + "...";

  return (
    <div>
      <pre className={`whitespace-pre-wrap break-words text-sm ${className}`}>
        {display}
      </pre>
      {needsTruncation && !expanded && (
        <button
          onClick={() => {
            expandedCache.set(cacheKey, true);
            setExpanded(true);
          }}
          className="text-xs text-blue-500 hover:text-blue-600 dark:text-blue-400 mt-1 font-medium"
        >
          展开完整内容 ({text.length} 字符)
        </button>
      )}
      {expanded && (
        <button
          onClick={() => {
            expandedCache.set(cacheKey, false);
            setExpanded(false);
          }}
          className="text-xs text-gray-400 hover:text-gray-500 dark:text-gray-500 mt-1"
        >
          收起
        </button>
      )}
    </div>
  );
}

function ChatBubble({ msg }: { msg: ChatMessage }) {
  if (msg.role === "user") {
    return (
      <div className="flex justify-end mb-3">
        <div className="max-w-[75%] flex flex-col items-end gap-1">
          <span className="text-xs text-blue-600 dark:text-blue-400 font-medium">
            You
          </span>
          <div className="bg-blue-500 text-white rounded-2xl rounded-br-md px-4 py-2.5">
            <TruncatableText text={msg.content} className="text-white" />
          </div>
        </div>
      </div>
    );
  }

  if (msg.role === "thinking") {
    return (
      <div className="flex justify-start mb-2">
        <details className="max-w-[75%] border border-amber-200 dark:border-amber-700/50 rounded-lg bg-amber-50/50 dark:bg-amber-900/10">
          <summary className="text-xs text-amber-600 dark:text-amber-400 font-medium cursor-pointer px-3 py-1.5 select-none">
            思考
          </summary>
          <div className="border-t border-amber-200 dark:border-amber-700/30">
            <TruncatableText
              text={msg.content}
              className="text-amber-700 dark:text-amber-300 p-3 max-h-48 overflow-y-auto text-xs"
            />
          </div>
        </details>
      </div>
    );
  }

  // assistant
  return (
    <div className="flex justify-start mb-3">
      <div className="max-w-[75%] flex flex-col items-start gap-1">
        <span className="text-xs text-green-600 dark:text-green-400 font-medium">
          Assistant
        </span>
        <div className="bg-gray-100 dark:bg-base-200 text-gray-900 dark:text-gray-100 rounded-2xl rounded-bl-md px-4 py-2.5">
          <TruncatableText
            text={msg.content}
            className="text-gray-900 dark:text-gray-100"
          />
        </div>
      </div>
    </div>
  );
}

// --- Main Component ---

interface LlmLogViewerProps {
  isOpen: boolean;
  onClose: () => void;
}

export default function LlmLogViewer({ isOpen, onClose }: LlmLogViewerProps) {
  const [traces, setTraces] = useState<TraceSummary[]>([]);
  const [selectedTrace, setSelectedTrace] = useState<string | null>(null);
  const [detail, setDetail] = useState<TraceDetail | null>(null);
  const [loadingTraces, setLoadingTraces] = useState(false);
  const [loadingDetail, setLoadingDetail] = useState(false);
  const [deletingTrace, setDeletingTrace] = useState<string | null>(null);

  const loadTraces = useCallback(async () => {
    setLoadingTraces(true);
    try {
      const result = await invoke<TraceSummary[]>("get_llm_log_traces");
      setTraces(result);
    } catch (err) {
      console.error("Failed to load LLM traces:", err);
    } finally {
      setLoadingTraces(false);
    }
  }, []);

  const loadDetail = useCallback(async (traceId: string) => {
    setSelectedTrace(traceId);
    setDetail(null);
    setLoadingDetail(true);
    try {
      const result = await invoke<TraceDetail>("get_llm_log_detail", {
        traceId,
      });
      setDetail(result);
    } catch (err) {
      console.error("Failed to load LLM trace detail:", err);
    } finally {
      setLoadingDetail(false);
    }
  }, []);

  const deleteTrace = useCallback(
    async (traceId: string, e: React.MouseEvent) => {
      e.stopPropagation();
      setDeletingTrace(traceId);
      try {
        await invoke("delete_llm_log_trace", { traceId });
        // Remove from local list
        setTraces((prev) => prev.filter((t) => t.trace_id !== traceId));
        // Clear selected detail if deleting current
        if (selectedTrace === traceId) {
          setSelectedTrace(null);
          setDetail(null);
        }
      } catch (err) {
        console.error("Failed to delete trace:", err);
      } finally {
        setDeletingTrace(null);
      }
    },
    [selectedTrace],
  );

  useEffect(() => {
    if (isOpen) {
      loadTraces();
    }
  }, [isOpen, loadTraces]);

  // Auto-refresh traces every 5 seconds when viewer is open
  useEffect(() => {
    if (!isOpen) return;
    const interval = setInterval(loadTraces, 5000);
    return () => clearInterval(interval);
  }, [isOpen, loadTraces]);

  const chatMessages = detail ? buildChatMessages(detail) : [];
  const selectedSummary = traces.find((t) => t.trace_id === selectedTrace);

  if (!isOpen) return null;

  return (
    <div className="fixed inset-0 z-[9999] flex items-center justify-center bg-black/40">
      <div className="bg-white dark:bg-base-100 rounded-xl shadow-2xl w-[95vw] max-w-6xl h-[85vh] flex flex-col overflow-hidden">
        {/* Header */}
        <div className="flex items-center justify-between px-6 py-4 border-b border-gray-200 dark:border-base-300">
          <div>
            <h2 className="text-lg font-semibold text-gray-900 dark:text-gray-100">
              LLM 对话日志
            </h2>
            <p className="text-xs text-gray-500 dark:text-gray-400">
              共 {traces.length} 条记录
            </p>
          </div>
          <button
            onClick={onClose}
            className="p-2 hover:bg-gray-100 dark:hover:bg-base-200 rounded-lg transition-colors"
          >
            <svg
              className="w-5 h-5 text-gray-500"
              fill="none"
              viewBox="0 0 24 24"
              stroke="currentColor"
            >
              <path
                strokeLinecap="round"
                strokeLinejoin="round"
                strokeWidth={2}
                d="M6 18L18 6M6 6l12 12"
              />
            </svg>
          </button>
        </div>

        {/* Body */}
        <div className="flex flex-1 overflow-hidden">
          {/* Left Panel: Trace List */}
          <div className="w-72 border-r border-gray-200 dark:border-base-300 flex flex-col">
            <div className="px-4 py-3 border-b border-gray-100 dark:border-base-300 bg-gray-50 dark:bg-base-200/50">
              <span className="text-xs font-medium text-gray-500 dark:text-gray-400 uppercase tracking-wide">
                Trace 列表
              </span>
            </div>
            <div className="flex-1 overflow-y-auto">
              {loadingTraces ? (
                <div className="flex items-center justify-center py-8">
                  <span className="loading loading-spinner loading-sm text-gray-400"></span>
                </div>
              ) : traces.length === 0 ? (
                <div className="px-4 py-8 text-center text-sm text-gray-400">
                  暂无记录
                </div>
              ) : (
                <ul className="divide-y divide-gray-100 dark:divide-base-300">
                  {traces.map((t) => (
                    <li
                      key={t.trace_id}
                      onClick={() => loadDetail(t.trace_id)}
                      className={`px-4 py-3 cursor-pointer transition-colors hover:bg-gray-50 dark:hover:bg-base-200/50 group ${
                        selectedTrace === t.trace_id
                          ? "bg-blue-50 dark:bg-blue-900/20 border-l-2 border-blue-500"
                          : ""
                      }`}
                    >
                      <div className="flex items-center justify-between gap-2">
                        <div className="flex items-center gap-2 min-w-0">
                          <code className="text-xs font-mono font-semibold text-gray-700 dark:text-gray-300 truncate">
                            {t.trace_id}
                          </code>
                          <span className="text-[10px] px-1.5 py-0.5 rounded bg-gray-100 dark:bg-base-300 text-gray-500 dark:text-gray-400 shrink-0">
                            {t.stages.length}/4
                          </span>
                        </div>
                        <button
                          onClick={(e) => deleteTrace(t.trace_id, e)}
                          disabled={deletingTrace === t.trace_id}
                          className="shrink-0 p-1 text-gray-400 hover:text-red-500 dark:text-gray-500 dark:hover:text-red-400 transition-colors opacity-0 group-hover:opacity-100 rounded"
                          title="删除此 trace 的详情文件"
                        >
                          {deletingTrace === t.trace_id ? (
                            <svg
                              className="w-3.5 h-3.5 animate-spin"
                              fill="none"
                              viewBox="0 0 24 24"
                            >
                              <circle
                                className="opacity-25"
                                cx="12"
                                cy="12"
                                r="10"
                                stroke="currentColor"
                                strokeWidth="4"
                              />
                              <path
                                className="opacity-75"
                                fill="currentColor"
                                d="M4 12a8 8 0 018-8V0C5.373 0 0 5.373 0 12h4zm2 5.291A7.962 7.962 0 014 12H0c0 3.042 1.135 5.824 3 7.938l3-2.647z"
                              />
                            </svg>
                          ) : (
                            <svg
                              className="w-3.5 h-3.5"
                              fill="none"
                              viewBox="0 0 24 24"
                              stroke="currentColor"
                            >
                              <path
                                strokeLinecap="round"
                                strokeLinejoin="round"
                                strokeWidth={2}
                                d="M6 18L18 6M6 6l12 12"
                              />
                            </svg>
                          )}
                        </button>
                      </div>
                      {t.model && (
                        <div className="text-xs text-gray-500 dark:text-gray-400 mt-1 truncate">
                          {t.model}
                        </div>
                      )}
                      <div className="text-[10px] text-gray-400 dark:text-gray-500 mt-0.5">
                        {t.timestamp
                          ? new Date(t.timestamp).toLocaleTimeString('en-GB', { hour: '2-digit', minute: '2-digit', second: '2-digit' })
                          : ""}
                      </div>
                    </li>
                  ))}
                </ul>
              )}
            </div>
          </div>

          {/* Right Panel: Conversation View */}
          <div className="flex-1 flex flex-col overflow-hidden">
            {selectedTrace ? (
              <>
                {/* Detail Header */}
                <div className="px-6 py-3 border-b border-gray-100 dark:border-base-300 bg-gray-50 dark:bg-base-200/30 flex items-center gap-3">
                  <code className="text-sm font-mono font-semibold text-gray-700 dark:text-gray-300">
                    {selectedTrace}
                  </code>
                  {selectedSummary?.model && (
                    <span className="text-xs px-2 py-0.5 rounded bg-green-100 dark:bg-green-900/30 text-green-700 dark:text-green-400">
                      {selectedSummary.model}
                    </span>
                  )}
                  {loadingDetail && (
                    <span className="loading loading-spinner loading-xs text-gray-400"></span>
                  )}
                </div>

                {/* Messages */}
                <div className="flex-1 overflow-y-auto px-6 py-4">
                  {loadingDetail ? (
                    <div className="flex items-center justify-center py-16">
                      <span className="loading loading-spinner loading-md text-gray-400"></span>
                      <span className="ml-3 text-sm text-gray-400">
                        加载中...
                      </span>
                    </div>
                  ) : chatMessages.length === 0 ? (
                    <div className="flex items-center justify-center py-16 text-sm text-gray-400">
                      无对话内容
                    </div>
                  ) : (
                    <>
                      <div className="text-xs text-gray-400 dark:text-gray-500 mb-4 font-medium">
                        共 {chatMessages.length} 条消息
                      </div>
                      {chatMessages.map((msg, i) => (
                        <ChatBubble key={i} msg={msg} />
                      ))}
                    </>
                  )}
                </div>
              </>
            ) : (
              <div className="flex-1 flex items-center justify-center text-gray-400 dark:text-gray-500">
                <div className="text-center">
                  <svg
                    className="w-12 h-12 mx-auto mb-3 opacity-50"
                    fill="none"
                    viewBox="0 0 24 24"
                    stroke="currentColor"
                  >
                    <path
                      strokeLinecap="round"
                      strokeLinejoin="round"
                      strokeWidth={1.5}
                      d="M8 12h.01M12 12h.01M16 12h.01M21 12c0 4.418-4.03 8-9 8a9.863 9.863 0 01-4.255-.949L3 20l1.395-3.72C3.512 15.042 3 13.574 3 12c0-4.418 4.03-8 9-8s9 3.582 9 8z"
                    />
                  </svg>
                  <p className="text-sm">选择左侧 Trace 查看对话详情</p>
                </div>
              </div>
            )}
          </div>
        </div>
      </div>
    </div>
  );
}
