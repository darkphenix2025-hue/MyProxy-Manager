use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

const SOURCE_VALUE: &str = "COALESCE(NULLIF(TRIM(account_email), ''), NULLIF(TRIM(provider_name), ''), NULLIF(TRIM(username), ''))";
const SOURCE_LABEL: &str = "COALESCE(NULLIF(TRIM(account_email), ''), NULLIF(TRIM(provider_name), ''), NULLIF(TRIM(username), ''), 'unknown')";
const MODEL_VALUE: &str = "COALESCE(NULLIF(TRIM(upstream_model), ''), NULLIF(TRIM(mapped_model), ''), NULLIF(TRIM(model), ''))";
const MODEL_LABEL: &str = "COALESCE(NULLIF(TRIM(upstream_model), ''), NULLIF(TRIM(mapped_model), ''), NULLIF(TRIM(model), ''), 'unknown')";
const INPUT_TOKENS: &str = "MAX(COALESCE(input_tokens, 0), 0)";
const OUTPUT_TOKENS: &str = "MAX(COALESCE(output_tokens, 0), 0)";
const SUCCESS: &str = "CASE WHEN status >= 200 AND status < 400 THEN 1 ELSE 0 END";
const FAILURE: &str = "CASE WHEN status >= 200 AND status < 400 THEN 0 ELSE 1 END";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenStatsAggregated {
    pub period: String,
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pub total_tokens: u64,
    pub request_count: u64,
    pub success_count: u64,
    pub error_count: u64,
    pub average_duration_ms: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountTokenStats {
    /// Compatibility field: this now represents an authenticated account when
    /// available, otherwise the configured provider used by the request.
    pub account_email: String,
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pub total_tokens: u64,
    pub request_count: u64,
    pub success_count: u64,
    pub error_count: u64,
    pub average_duration_ms: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenStatsSummary {
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pub total_tokens: u64,
    pub total_requests: u64,
    pub successful_requests: u64,
    pub failed_requests: u64,
    pub success_rate: f64,
    pub average_duration_ms: f64,
    pub unique_sources: u64,
    pub unique_accounts: u64,
    pub unique_providers: u64,
    pub unique_models: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelTokenStats {
    pub model: String,
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pub total_tokens: u64,
    pub request_count: u64,
    pub success_count: u64,
    pub error_count: u64,
    pub average_duration_ms: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelTrendPoint {
    pub period: String,
    pub model_data: HashMap<String, u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountTrendPoint {
    pub period: String,
    pub account_data: HashMap<String, u64>,
}

/// Path of the retired standalone database. It is kept only so upgrades can
/// still open old installations and the legacy clear endpoint remains safe.
pub(crate) fn get_db_path() -> Result<PathBuf, String> {
    let data_dir = crate::modules::account::get_data_dir()?;
    Ok(data_dir.join("token_stats.db"))
}

fn connect_legacy_db() -> Result<Connection, String> {
    let conn = Connection::open(get_db_path()?).map_err(|e| e.to_string())?;
    configure_connection(&conn)?;
    Ok(conn)
}

fn configure_connection(conn: &Connection) -> Result<(), String> {
    conn.pragma_update(None, "journal_mode", "WAL")
        .map_err(|e| e.to_string())?;
    conn.pragma_update(None, "busy_timeout", 5000)
        .map_err(|e| e.to_string())?;
    conn.pragma_update(None, "synchronous", "NORMAL")
        .map_err(|e| e.to_string())?;
    Ok(())
}

fn connect_stats_db() -> Result<Connection, String> {
    crate::modules::proxy_db::init_db()?;
    let conn = Connection::open(crate::modules::proxy_db::get_proxy_db_path()?)
        .map_err(|e| e.to_string())?;
    configure_connection(&conn)?;
    Ok(conn)
}

/// Retain the legacy schema for backwards-compatible startup. Dashboard and
/// Token Statistics queries use the lightweight `request_statistics` table in
/// proxy_logs.db so traffic-log retention cannot erase historical usage.
pub fn init_db() -> Result<(), String> {
    let conn = connect_legacy_db()?;
    conn.execute(
        "CREATE TABLE IF NOT EXISTS token_usage (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            timestamp INTEGER NOT NULL,
            account_email TEXT NOT NULL,
            model TEXT NOT NULL,
            input_tokens INTEGER NOT NULL DEFAULT 0,
            output_tokens INTEGER NOT NULL DEFAULT 0,
            total_tokens INTEGER NOT NULL DEFAULT 0
        )",
        [],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

pub fn clear_stats() -> Result<(), String> {
    crate::modules::proxy_db::clear_statistics()?;

    // Keep the retired store consistent for installations that still contain
    // records from an older application version.
    let conn = connect_legacy_db()?;
    conn.execute("DELETE FROM token_usage", [])
        .map_err(|e| e.to_string())?;
    Ok(())
}

fn positive_period(value: i64) -> i64 {
    value.max(1)
}

fn cutoff_hours(hours: i64) -> i64 {
    chrono::Utc::now()
        .timestamp_millis()
        .saturating_sub(positive_period(hours).saturating_mul(3_600_000))
}

fn cutoff_days(days: i64) -> i64 {
    chrono::Utc::now()
        .timestamp_millis()
        .saturating_sub(positive_period(days).saturating_mul(86_400_000))
}

fn cutoff_weeks(weeks: i64) -> i64 {
    cutoff_days(positive_period(weeks).saturating_mul(7))
}

fn as_u64(value: i64) -> u64 {
    value.max(0) as u64
}

fn aggregated_query(
    conn: &Connection,
    cutoff_ms: i64,
    period_expression: &str,
) -> Result<Vec<TokenStatsAggregated>, String> {
    let sql = format!(
        "SELECT {period_expression} AS period,
                COALESCE(SUM({INPUT_TOKENS}), 0),
                COALESCE(SUM({OUTPUT_TOKENS}), 0),
                COALESCE(SUM({INPUT_TOKENS} + {OUTPUT_TOKENS}), 0),
                COUNT(*),
                COALESCE(SUM({SUCCESS}), 0),
                COALESCE(SUM({FAILURE}), 0),
                COALESCE(AVG(MAX(COALESCE(duration, 0), 0)), 0.0)
         FROM request_statistics
         WHERE timestamp >= ?1
         GROUP BY period
         ORDER BY period ASC"
    );
    let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([cutoff_ms], |row| {
            Ok(TokenStatsAggregated {
                period: row.get(0)?,
                total_input_tokens: as_u64(row.get(1)?),
                total_output_tokens: as_u64(row.get(2)?),
                total_tokens: as_u64(row.get(3)?),
                request_count: as_u64(row.get(4)?),
                success_count: as_u64(row.get(5)?),
                error_count: as_u64(row.get(6)?),
                average_duration_ms: row.get(7)?,
            })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())
}

fn get_hourly_stats_from_conn(
    conn: &Connection,
    cutoff_ms: i64,
) -> Result<Vec<TokenStatsAggregated>, String> {
    aggregated_query(
        conn,
        cutoff_ms,
        "strftime('%Y-%m-%d %H:00', datetime(timestamp / 1000, 'unixepoch', 'localtime'))",
    )
}

fn get_daily_stats_from_conn(
    conn: &Connection,
    cutoff_ms: i64,
) -> Result<Vec<TokenStatsAggregated>, String> {
    aggregated_query(
        conn,
        cutoff_ms,
        "strftime('%Y-%m-%d', datetime(timestamp / 1000, 'unixepoch', 'localtime'))",
    )
}

fn get_weekly_stats_from_conn(
    conn: &Connection,
    cutoff_ms: i64,
) -> Result<Vec<TokenStatsAggregated>, String> {
    aggregated_query(
        conn,
        cutoff_ms,
        "strftime('%Y-W%W', datetime(timestamp / 1000, 'unixepoch', 'localtime'))",
    )
}

pub fn get_hourly_stats(hours: i64) -> Result<Vec<TokenStatsAggregated>, String> {
    get_hourly_stats_from_conn(&connect_stats_db()?, cutoff_hours(hours))
}

pub fn get_daily_stats(days: i64) -> Result<Vec<TokenStatsAggregated>, String> {
    get_daily_stats_from_conn(&connect_stats_db()?, cutoff_days(days))
}

pub fn get_weekly_stats(weeks: i64) -> Result<Vec<TokenStatsAggregated>, String> {
    get_weekly_stats_from_conn(&connect_stats_db()?, cutoff_weeks(weeks))
}

fn get_account_stats_from_conn(
    conn: &Connection,
    cutoff_ms: i64,
) -> Result<Vec<AccountTokenStats>, String> {
    let sql = format!(
        "SELECT {SOURCE_LABEL} AS source,
                COALESCE(SUM({INPUT_TOKENS}), 0),
                COALESCE(SUM({OUTPUT_TOKENS}), 0),
                COALESCE(SUM({INPUT_TOKENS} + {OUTPUT_TOKENS}), 0),
                COUNT(*),
                COALESCE(SUM({SUCCESS}), 0),
                COALESCE(SUM({FAILURE}), 0),
                COALESCE(AVG(MAX(COALESCE(duration, 0), 0)), 0.0)
         FROM request_statistics
         WHERE timestamp >= ?1
         GROUP BY source
         ORDER BY 4 DESC, 5 DESC, source ASC"
    );
    let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([cutoff_ms], |row| {
            Ok(AccountTokenStats {
                account_email: row.get(0)?,
                total_input_tokens: as_u64(row.get(1)?),
                total_output_tokens: as_u64(row.get(2)?),
                total_tokens: as_u64(row.get(3)?),
                request_count: as_u64(row.get(4)?),
                success_count: as_u64(row.get(5)?),
                error_count: as_u64(row.get(6)?),
                average_duration_ms: row.get(7)?,
            })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())
}

pub fn get_account_stats(hours: i64) -> Result<Vec<AccountTokenStats>, String> {
    get_account_stats_from_conn(&connect_stats_db()?, cutoff_hours(hours))
}

fn get_summary_stats_from_conn(
    conn: &Connection,
    cutoff_ms: i64,
) -> Result<TokenStatsSummary, String> {
    let sql = format!(
        "SELECT COUNT(*),
                COALESCE(SUM({SUCCESS}), 0),
                COALESCE(SUM({FAILURE}), 0),
                COALESCE(SUM({INPUT_TOKENS}), 0),
                COALESCE(SUM({OUTPUT_TOKENS}), 0),
                COALESCE(SUM({INPUT_TOKENS} + {OUTPUT_TOKENS}), 0),
                COALESCE(AVG(MAX(COALESCE(duration, 0), 0)), 0.0),
                COUNT(DISTINCT {SOURCE_VALUE}),
                COUNT(DISTINCT NULLIF(TRIM(account_email), '')),
                COUNT(DISTINCT NULLIF(TRIM(provider_name), '')),
                COUNT(DISTINCT {MODEL_VALUE})
         FROM request_statistics
         WHERE timestamp >= ?1"
    );
    let (
        total_requests,
        successful_requests,
        failed_requests,
        total_input_tokens,
        total_output_tokens,
        total_tokens,
        average_duration_ms,
        unique_sources,
        unique_accounts,
        unique_providers,
        unique_models,
    ): (i64, i64, i64, i64, i64, i64, f64, i64, i64, i64, i64) = conn
        .query_row(&sql, [cutoff_ms], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
                row.get(6)?,
                row.get(7)?,
                row.get(8)?,
                row.get(9)?,
                row.get(10)?,
            ))
        })
        .map_err(|e| e.to_string())?;

    let success_rate = if total_requests > 0 {
        successful_requests as f64 * 100.0 / total_requests as f64
    } else {
        0.0
    };

    Ok(TokenStatsSummary {
        total_input_tokens: as_u64(total_input_tokens),
        total_output_tokens: as_u64(total_output_tokens),
        total_tokens: as_u64(total_tokens),
        total_requests: as_u64(total_requests),
        successful_requests: as_u64(successful_requests),
        failed_requests: as_u64(failed_requests),
        success_rate,
        average_duration_ms,
        unique_sources: as_u64(unique_sources),
        unique_accounts: as_u64(unique_accounts),
        unique_providers: as_u64(unique_providers),
        unique_models: as_u64(unique_models),
    })
}

pub fn get_summary_stats(hours: i64) -> Result<TokenStatsSummary, String> {
    get_summary_stats_from_conn(&connect_stats_db()?, cutoff_hours(hours))
}

fn get_model_stats_from_conn(
    conn: &Connection,
    cutoff_ms: i64,
) -> Result<Vec<ModelTokenStats>, String> {
    let sql = format!(
        "SELECT {MODEL_LABEL} AS actual_model,
                COALESCE(SUM({INPUT_TOKENS}), 0),
                COALESCE(SUM({OUTPUT_TOKENS}), 0),
                COALESCE(SUM({INPUT_TOKENS} + {OUTPUT_TOKENS}), 0),
                COUNT(*),
                COALESCE(SUM({SUCCESS}), 0),
                COALESCE(SUM({FAILURE}), 0),
                COALESCE(AVG(MAX(COALESCE(duration, 0), 0)), 0.0)
         FROM request_statistics
         WHERE timestamp >= ?1
         GROUP BY actual_model
         ORDER BY 4 DESC, 5 DESC, actual_model ASC"
    );
    let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([cutoff_ms], |row| {
            Ok(ModelTokenStats {
                model: row.get(0)?,
                total_input_tokens: as_u64(row.get(1)?),
                total_output_tokens: as_u64(row.get(2)?),
                total_tokens: as_u64(row.get(3)?),
                request_count: as_u64(row.get(4)?),
                success_count: as_u64(row.get(5)?),
                error_count: as_u64(row.get(6)?),
                average_duration_ms: row.get(7)?,
            })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())
}

pub fn get_model_stats(hours: i64) -> Result<Vec<ModelTokenStats>, String> {
    get_model_stats_from_conn(&connect_stats_db()?, cutoff_hours(hours))
}

fn model_trend_from_conn(
    conn: &Connection,
    cutoff_ms: i64,
    period_expression: &str,
) -> Result<Vec<ModelTrendPoint>, String> {
    let sql = format!(
        "SELECT {period_expression} AS period,
                {MODEL_LABEL} AS actual_model,
                COALESCE(SUM({INPUT_TOKENS} + {OUTPUT_TOKENS}), 0)
         FROM request_statistics
         WHERE timestamp >= ?1
         GROUP BY period, actual_model
         ORDER BY period ASC"
    );
    let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([cutoff_ms], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                as_u64(row.get(2)?),
            ))
        })
        .map_err(|e| e.to_string())?;
    let mut trend: BTreeMap<String, HashMap<String, u64>> = BTreeMap::new();
    for row in rows {
        let (period, model, tokens) = row.map_err(|e| e.to_string())?;
        trend.entry(period).or_default().insert(model, tokens);
    }
    Ok(trend
        .into_iter()
        .map(|(period, model_data)| ModelTrendPoint { period, model_data })
        .collect())
}

pub fn get_model_trend_hourly(hours: i64) -> Result<Vec<ModelTrendPoint>, String> {
    model_trend_from_conn(
        &connect_stats_db()?,
        cutoff_hours(hours),
        "strftime('%Y-%m-%d %H:00', datetime(timestamp / 1000, 'unixepoch', 'localtime'))",
    )
}

pub fn get_model_trend_daily(days: i64) -> Result<Vec<ModelTrendPoint>, String> {
    model_trend_from_conn(
        &connect_stats_db()?,
        cutoff_days(days),
        "strftime('%Y-%m-%d', datetime(timestamp / 1000, 'unixepoch', 'localtime'))",
    )
}

fn account_trend_from_conn(
    conn: &Connection,
    cutoff_ms: i64,
    period_expression: &str,
) -> Result<Vec<AccountTrendPoint>, String> {
    let sql = format!(
        "SELECT {period_expression} AS period,
                {SOURCE_LABEL} AS source,
                COALESCE(SUM({INPUT_TOKENS} + {OUTPUT_TOKENS}), 0)
         FROM request_statistics
         WHERE timestamp >= ?1
         GROUP BY period, source
         ORDER BY period ASC"
    );
    let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([cutoff_ms], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                as_u64(row.get(2)?),
            ))
        })
        .map_err(|e| e.to_string())?;
    let mut trend: BTreeMap<String, HashMap<String, u64>> = BTreeMap::new();
    for row in rows {
        let (period, source, tokens) = row.map_err(|e| e.to_string())?;
        trend.entry(period).or_default().insert(source, tokens);
    }
    Ok(trend
        .into_iter()
        .map(|(period, account_data)| AccountTrendPoint {
            period,
            account_data,
        })
        .collect())
}

pub fn get_account_trend_hourly(hours: i64) -> Result<Vec<AccountTrendPoint>, String> {
    account_trend_from_conn(
        &connect_stats_db()?,
        cutoff_hours(hours),
        "strftime('%Y-%m-%d %H:00', datetime(timestamp / 1000, 'unixepoch', 'localtime'))",
    )
}

pub fn get_account_trend_daily(days: i64) -> Result<Vec<AccountTrendPoint>, String> {
    account_trend_from_conn(
        &connect_stats_db()?,
        cutoff_days(days),
        "strftime('%Y-%m-%d', datetime(timestamp / 1000, 'unixepoch', 'localtime'))",
    )
}

#[cfg(test)]
mod tests {
    use super::{
        get_account_stats_from_conn, get_hourly_stats_from_conn, get_model_stats_from_conn,
        get_summary_stats_from_conn,
    };
    use rusqlite::{params, Connection};

    const NOW_MS: i64 = 1_800_000_000_000;
    const CUTOFF_MS: i64 = NOW_MS - 3_600_000;

    fn test_connection() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE request_logs (
                id TEXT PRIMARY KEY,
                timestamp INTEGER,
                status INTEGER,
                duration INTEGER,
                model TEXT,
                mapped_model TEXT,
                upstream_model TEXT,
                input_tokens INTEGER,
                output_tokens INTEGER,
                account_email TEXT,
                provider_name TEXT,
                username TEXT
            );
            CREATE TABLE request_statistics (
                id TEXT PRIMARY KEY,
                timestamp INTEGER,
                status INTEGER,
                duration INTEGER,
                model TEXT,
                mapped_model TEXT,
                upstream_model TEXT,
                input_tokens INTEGER,
                output_tokens INTEGER,
                account_email TEXT,
                provider_name TEXT,
                username TEXT
            );",
        )
        .unwrap();

        let rows = [
            (
                "provider-success",
                NOW_MS - 3_000,
                200,
                100,
                "claude-alias",
                Some("big/gpt-5.6-luna"),
                Some("gpt-5.6-luna"),
                Some(100),
                Some(20),
                None,
                Some("BIGMODEL"),
                None,
            ),
            (
                "provider-error",
                NOW_MS - 2_000,
                429,
                300,
                "claude-alias",
                Some("big/gpt-5.6-luna"),
                Some("gpt-5.6-luna"),
                None,
                None,
                None,
                Some("BIGMODEL"),
                None,
            ),
            (
                "account-success",
                NOW_MS - 1_000,
                204,
                200,
                "background-alias",
                Some("codex/o3"),
                Some("o3"),
                Some(50),
                Some(25),
                Some("user@example.com"),
                Some("CODEX"),
                Some("local-user"),
            ),
            (
                "outside-range",
                CUTOFF_MS - 1,
                200,
                999,
                "old-model",
                None,
                None,
                Some(9_999),
                Some(9_999),
                None,
                Some("OLD"),
                None,
            ),
        ];

        for row in rows {
            for table in ["request_logs", "request_statistics"] {
                conn.execute(
                    &format!(
                        "INSERT INTO {table} (
                            id, timestamp, status, duration, model, mapped_model, upstream_model,
                            input_tokens, output_tokens, account_email, provider_name, username
                         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)"
                    ),
                    params![
                        row.0, row.1, row.2, row.3, row.4, row.5, row.6, row.7, row.8, row.9,
                        row.10, row.11
                    ],
                )
                .unwrap();
            }
        }
        conn
    }

    #[test]
    fn statistics_survive_traffic_log_cleanup() {
        let conn = test_connection();
        conn.execute("DELETE FROM request_logs", []).unwrap();

        let summary = get_summary_stats_from_conn(&conn, CUTOFF_MS).unwrap();
        let hourly = get_hourly_stats_from_conn(&conn, CUTOFF_MS).unwrap();
        let accounts = get_account_stats_from_conn(&conn, CUTOFF_MS).unwrap();
        let models = get_model_stats_from_conn(&conn, CUTOFF_MS).unwrap();

        assert_eq!(summary.total_requests, 3);
        assert_eq!(summary.total_tokens, 195);
        assert_eq!(
            hourly.iter().map(|point| point.request_count).sum::<u64>(),
            3
        );
        assert_eq!(
            accounts
                .iter()
                .map(|source| source.request_count)
                .sum::<u64>(),
            3
        );
        assert_eq!(
            models.iter().map(|model| model.request_count).sum::<u64>(),
            3
        );
    }

    #[test]
    fn summary_counts_all_requests_and_uses_consistent_success_token_and_latency_rules() {
        let summary = get_summary_stats_from_conn(&test_connection(), CUTOFF_MS).unwrap();
        assert_eq!(summary.total_requests, 3);
        assert_eq!(summary.successful_requests, 2);
        assert_eq!(summary.failed_requests, 1);
        assert!((summary.success_rate - 66.666_666).abs() < 0.001);
        assert!((summary.average_duration_ms - 200.0).abs() < f64::EPSILON);
        assert_eq!(summary.total_input_tokens, 150);
        assert_eq!(summary.total_output_tokens, 45);
        assert_eq!(summary.total_tokens, 195);
        assert_eq!(summary.unique_sources, 2);
        assert_eq!(summary.unique_accounts, 1);
        assert_eq!(summary.unique_providers, 2);
        assert_eq!(summary.unique_models, 2);
    }

    #[test]
    fn source_stats_fall_back_to_provider_but_prefer_an_authenticated_account() {
        let stats = get_account_stats_from_conn(&test_connection(), CUTOFF_MS).unwrap();
        assert_eq!(stats.len(), 2);
        assert_eq!(stats[0].account_email, "BIGMODEL");
        assert_eq!(stats[0].request_count, 2);
        assert_eq!(stats[0].total_tokens, 120);
        assert_eq!(stats[1].account_email, "user@example.com");
        assert_eq!(stats[1].request_count, 1);
        assert_eq!(stats[1].total_tokens, 75);
    }

    #[test]
    fn model_stats_use_the_actual_upstream_model_and_include_failed_requests() {
        let stats = get_model_stats_from_conn(&test_connection(), CUTOFF_MS).unwrap();
        assert_eq!(stats.len(), 2);
        assert_eq!(stats[0].model, "gpt-5.6-luna");
        assert_eq!(stats[0].request_count, 2);
        assert_eq!(stats[0].total_tokens, 120);
        assert_eq!(stats[1].model, "o3");
        assert_eq!(stats[1].request_count, 1);
        assert_eq!(stats[1].total_tokens, 75);
    }
}
