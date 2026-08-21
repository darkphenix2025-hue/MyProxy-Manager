export interface CodexConnection {
  readonly identity: {
    readonly display_name?: string;
    readonly email?: string;
    readonly workspace?: string;
    readonly metadata?: Readonly<Record<string, unknown>>;
  };
  readonly credential: {
    readonly id: string;
    readonly auth_kind: string;
    readonly lifecycle: string;
    readonly expires_at?: number;
    readonly fingerprint: string;
  };
  readonly connection: {
    readonly id: string;
    readonly provider_kind: string;
    readonly endpoint: string;
    readonly status: string;
    readonly enabled: boolean;
    readonly config?: Readonly<Record<string, unknown>>;
  };
}

export interface CodexModel {
  readonly id: string;
  readonly display_name?: string;
  readonly owned_by?: string;
}

export interface CodexQuotaWindow {
  readonly used_percent?: number;
  readonly limit_window_seconds?: number;
  readonly reset_after_seconds?: number;
  readonly reset_at?: number;
}

export interface CodexQuotaLimit {
  readonly allowed?: boolean;
  readonly limit_reached?: boolean;
  readonly primary_window?: CodexQuotaWindow;
  readonly secondary_window?: CodexQuotaWindow;
}

export interface CodexAdditionalQuota {
  readonly limit_name?: string;
  readonly metered_feature?: string;
  readonly rate_limit?: CodexQuotaLimit;
}

export interface CodexResetCredit {
  readonly id?: string;
  readonly reset_type?: string;
  readonly status?: string;
  readonly granted_at?: number;
  readonly expires_at?: number;
}

export interface CodexQuotaSummary {
  readonly plan_type?: string;
  readonly subscription_expires_at?: number;
  readonly rate_limit?: CodexQuotaLimit;
  readonly code_review_rate_limit?: CodexQuotaLimit;
  readonly additional_rate_limits?: ReadonlyArray<CodexAdditionalQuota>;
  readonly reset_credits_available_count?: number;
  readonly reset_credits_applicable_available_count?: number;
  readonly reset_credits?: ReadonlyArray<CodexResetCredit>;
  readonly reset_credits_error?: string;
  readonly fetched_at: number;
}

export interface CodexLoginStart {
  readonly session_id: string;
  readonly authorization_url: string;
  readonly expires_at: number;
  readonly browser_opened?: boolean;
}

export type CodexLoginPhase = 'pending' | 'completed' | 'failed' | 'cancelled';

export interface CodexLoginStatus {
  readonly phase: CodexLoginPhase;
  readonly error_code?: string;
}
