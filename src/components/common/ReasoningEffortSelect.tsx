import { useMemo } from 'react';
import { cn } from '../../utils/cn';
import type { ProviderProtocol, RouteReasoningEffort } from '../../types/config';
import {
  findModelSelectionForValue,
  type ModelSelectionSource,
} from './ProviderModelSelect';

interface ReasoningEffortSelectProps {
  readonly value: string;
  readonly onChange: (value: string) => void;
  readonly target: string;
  readonly sources: ReadonlyArray<ModelSelectionSource>;
  readonly className?: string;
  readonly disabled?: boolean;
}

interface EffortOption {
  readonly value: RouteReasoningEffort;
  readonly label: string;
}

const COMMON_OPTIONS: ReadonlyArray<EffortOption> = [
  { value: 'low', label: '低 / low' },
  { value: 'medium', label: '中 / medium' },
  { value: 'high', label: '高 / high' },
];

function findProtocol(
  target: string,
  sources: ReadonlyArray<ModelSelectionSource>,
): ProviderProtocol | null {
  return findModelSelectionForValue(target, sources)?.protocol.protocol ?? null;
}

function findAllowedEfforts(
  target: string,
  sources: ReadonlyArray<ModelSelectionSource>,
): ReadonlyArray<string> | null {
  const resolved = findModelSelectionForValue(target, sources);
  if (!resolved) return null;
  return resolved.protocol.models.find((model) => model.value === resolved.model)?.reasoningEfforts ?? null;
}

function getOptions(
  protocol: ProviderProtocol | null,
  allowedEfforts: ReadonlyArray<string> | null,
): ReadonlyArray<EffortOption> {
  const defaultOptions: ReadonlyArray<EffortOption> = (() => {
  switch (protocol) {
    case 'codex_responses':
      return [
        { value: 'low', label: 'light' },
        { value: 'medium', label: 'medium' },
        { value: 'high', label: 'high' },
        { value: 'xhigh', label: 'extra high' },
        { value: 'max', label: 'max' },
        { value: 'ultra', label: 'ultra' },
      ];
    case 'anthropic_passthrough':
      return [
        { value: 'low', label: 'low' },
        { value: 'medium', label: 'medium' },
        { value: 'high', label: 'high' },
        { value: 'max', label: 'max' },
      ];
    case 'open_a_i_compatible':
      return [
        ...COMMON_OPTIONS,
        { value: 'xhigh', label: 'extra high / xhigh' },
      ];
    case 'gemini_v1_internal':
      return COMMON_OPTIONS;
    default:
      return COMMON_OPTIONS;
  }
  })();

  if (!allowedEfforts || allowedEfforts.length === 0) return defaultOptions;
  const allowed = new Set(allowedEfforts);
  return defaultOptions.filter((option) => allowed.has(option.value));
}

export default function ReasoningEffortSelect({
  value,
  onChange,
  target,
  sources,
  className,
  disabled = false,
}: ReasoningEffortSelectProps) {
  const options = useMemo(
    () => getOptions(findProtocol(target, sources), findAllowedEfforts(target, sources)),
    [sources, target],
  );
  const hasLegacyValue = value !== '' && !options.some((option) => option.value === value);

  return (
    <select
      aria-label="推理强度"
      className={cn(
        'select select-xs h-8 min-w-28 border-gray-300 bg-white text-[10px] dark:border-gray-600 dark:bg-gray-800',
        className,
      )}
      value={value}
      onChange={(event) => onChange(event.target.value)}
      disabled={disabled}
    >
      <option value="">默认（跟随请求）</option>
      {hasLegacyValue && <option value={value}>{value}</option>}
      {options.map((option) => (
        <option key={option.value} value={option.value}>
          {option.label}
        </option>
      ))}
    </select>
  );
}
