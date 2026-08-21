import { useEffect, useMemo, useState } from 'react';
import GroupedSelect, { type SelectOption } from './GroupedSelect';
import { cn } from '../../utils/cn';
import type { ProviderProtocol } from '../../types/config';

export interface ModelSelectionSource {
  readonly key: string;
  readonly label: string;
  readonly kind: 'provider' | 'account';
  /** Representative protocol kept for older consumers. */
  readonly protocol: ProviderProtocol;
  /** The prefix stored in a route target, without the trailing slash. */
  readonly targetPrefix: string;
  /** Legacy prefixes that should still be recognized when editing a route. */
  readonly targetPrefixes?: ReadonlyArray<string>;
  readonly models: ReadonlyArray<ModelSelectionOption>;
  /** Protocols shown as the second level of the selector. */
  readonly protocols?: ReadonlyArray<ModelSelectionProtocol>;
}

export interface ModelSelectionOption extends SelectOption {
  readonly reasoningEfforts?: ReadonlyArray<string>;
}

export interface ModelSelectionProtocol {
  readonly key: string;
  readonly label: string;
  readonly protocol: ProviderProtocol;
  /** Canonical prefix written to a newly created route target. */
  readonly targetPrefix: string;
  /** Older prefixes accepted when resolving an existing route target. */
  readonly targetPrefixes?: ReadonlyArray<string>;
  readonly models: ReadonlyArray<ModelSelectionOption>;
}

export interface ResolvedModelSelection {
  readonly source: ModelSelectionSource;
  readonly protocol: ModelSelectionProtocol;
  readonly model: string;
}

interface ProviderModelSelectProps {
  readonly value: string;
  readonly onChange: (value: string) => void;
  readonly sources: ReadonlyArray<ModelSelectionSource>;
  readonly placeholder?: string;
  readonly className?: string;
  readonly allowCustomInput?: boolean;
  readonly disabled?: boolean;
}

const MANUAL_SOURCE_KEY = '__manual__';

const PROTOCOL_LABELS: Record<ProviderProtocol, string> = {
  anthropic_passthrough: 'Anthropic',
  open_a_i_compatible: 'OpenAI',
  codex_responses: 'Codex',
  gemini_v1_internal: 'Gemini',
};

export function getModelSelectionProtocols(
  source: ModelSelectionSource,
): ReadonlyArray<ModelSelectionProtocol> {
  if (source.protocols && source.protocols.length > 0) return source.protocols;
  return [{
    key: `${source.key}:protocol:${source.protocol}`,
    label: PROTOCOL_LABELS[source.protocol],
    protocol: source.protocol,
    targetPrefix: source.targetPrefix,
    targetPrefixes: source.targetPrefixes,
    models: source.models,
  }];
}

export function getProviderProtocolLabel(protocol: ProviderProtocol): string {
  return PROTOCOL_LABELS[protocol];
}

export function findModelSelectionForValue(
  value: string,
  sources: ReadonlyArray<ModelSelectionSource>,
): ResolvedModelSelection | null {
  const candidates: Array<{
    source: ModelSelectionSource;
    protocol: ModelSelectionProtocol;
    prefix: string;
  }> = [];

  for (const source of sources) {
    for (const protocol of getModelSelectionProtocols(source)) {
      for (const prefix of [protocol.targetPrefix, ...(protocol.targetPrefixes ?? [])]) {
        if (prefix) candidates.push({ source, protocol, prefix });
      }
    }
    // A source-level alias is enough to keep an old route editable even when
    // the persisted config did not retain which protocol owned that alias.
    const defaultProtocol = getModelSelectionProtocols(source)[0];
    for (const prefix of source.targetPrefixes ?? []) {
      if (prefix && defaultProtocol) candidates.push({ source, protocol: defaultProtocol, prefix });
    }
  }

  const match = candidates
    .sort((left, right) => right.prefix.length - left.prefix.length)
    .find(({ prefix }) => value.startsWith(`${prefix}/`));
  if (!match) return null;
  return {
    source: match.source,
    protocol: match.protocol,
    model: value.slice(match.prefix.length + 1),
  };
}

export default function ProviderModelSelect({
  value,
  onChange,
  sources,
  placeholder = '选择模型',
  className,
  allowCustomInput = false,
  disabled = false,
}: ProviderModelSelectProps) {
  const [selectedSourceKey, setSelectedSourceKey] = useState<string>(sources[0]?.key ?? '');
  const [selectedProtocolKey, setSelectedProtocolKey] = useState<string>('');

  const parsedValue = useMemo(() => findModelSelectionForValue(value, sources), [sources, value]);
  const selectedSource = sources.find((source) => source.key === selectedSourceKey);
  const selectedProtocols = selectedSource ? getModelSelectionProtocols(selectedSource) : [];
  const selectedProtocol = selectedProtocols.find((protocol) => protocol.key === selectedProtocolKey)
    ?? selectedProtocols[0];
  const modelValue = parsedValue?.source.key === selectedSourceKey
    ? parsedValue.model
    : selectedSourceKey === MANUAL_SOURCE_KEY
      ? value
      : '';

  useEffect(() => {
    if (parsedValue) {
      setSelectedSourceKey(parsedValue.source.key);
      setSelectedProtocolKey(parsedValue.protocol.key);
      return;
    }
    if (!value) {
      if (!selectedSourceKey) {
        if (sources[0]) setSelectedSourceKey(sources[0].key);
        return;
      }
      if (selectedSourceKey !== MANUAL_SOURCE_KEY && !sources.some((source) => source.key === selectedSourceKey)) {
        if (sources[0]) setSelectedSourceKey(sources[0].key);
        return;
      }
      const source = sources.find((candidate) => candidate.key === selectedSourceKey);
      const protocols = source ? getModelSelectionProtocols(source) : [];
      if (protocols.length > 0 && !protocols.some((protocol) => protocol.key === selectedProtocolKey)) {
        setSelectedProtocolKey(protocols[0].key);
      }
      return;
    }
    setSelectedSourceKey(MANUAL_SOURCE_KEY);
    setSelectedProtocolKey('');
  }, [parsedValue, selectedProtocolKey, selectedSourceKey, sources, value]);

  const handleSourceChange = (sourceKey: string) => {
    setSelectedSourceKey(sourceKey);
    const source = sources.find((candidate) => candidate.key === sourceKey);
    setSelectedProtocolKey(source ? getModelSelectionProtocols(source)[0]?.key ?? '' : '');
    onChange('');
  };

  const handleProtocolChange = (protocolKey: string) => {
    setSelectedProtocolKey(protocolKey);
    onChange('');
  };

  const handleModelChange = (model: string) => {
    if (selectedSourceKey === MANUAL_SOURCE_KEY || !selectedSource) {
      onChange(model);
      return;
    }
    onChange(model ? `${selectedProtocol?.targetPrefix ?? selectedSource.targetPrefix}/${model}` : '');
  };

  return (
    <div className={cn('flex min-w-0 flex-wrap items-center gap-1.5', className)}>
      <select
        aria-label="供应商"
        className="select select-xs h-8 min-w-24 max-w-44 shrink-0 border-gray-300 bg-white text-[10px] dark:border-gray-600 dark:bg-gray-800"
        value={selectedSourceKey || sources[0]?.key || MANUAL_SOURCE_KEY}
        onChange={(event) => handleSourceChange(event.target.value)}
        disabled={disabled}
      >
        {(['provider', 'account'] as const).map((kind) => {
          const kindSources = sources.filter((source) => source.kind === kind);
          if (kindSources.length === 0) return null;
          return (
            <optgroup key={kind} label={kind === 'provider' ? '供应商' : '账号'}>
              {kindSources.map((source) => (
                <option key={source.key} value={source.key}>
                  {source.label}
                </option>
              ))}
            </optgroup>
          );
        })}
        <option value={MANUAL_SOURCE_KEY}>手动输入</option>
      </select>
      {selectedSource && (
        <select
          aria-label="协议"
          className="select select-xs h-8 min-w-20 max-w-32 shrink-0 border-gray-300 bg-white text-[10px] dark:border-gray-600 dark:bg-gray-800"
          value={selectedProtocol?.key ?? ''}
          onChange={(event) => handleProtocolChange(event.target.value)}
          disabled={disabled || selectedProtocols.length <= 1}
          title={selectedProtocols.length > 1 ? '先选择协议，再选择模型' : undefined}
        >
          {selectedProtocols.map((protocol) => (
            <option key={protocol.key} value={protocol.key}>
              {protocol.label}
            </option>
          ))}
        </select>
      )}
      <GroupedSelect
        value={modelValue}
        onChange={handleModelChange}
        options={[...(selectedProtocol?.models ?? selectedSource?.models ?? [])]}
        placeholder={placeholder}
        className="min-w-[8rem] flex-1"
        allowCustomInput={allowCustomInput}
        disabled={disabled}
      />
    </div>
  );
}
