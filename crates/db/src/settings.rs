//! Runtime-editable integer settings stored in the `settings` table.

use crate::{DbError, DbResult};
use serde_json::Value;
use sqlx::PgExecutor;

/// Keys the admin page may edit.
pub const KNOWN_KEYS: &[&str] = &[
    "catalog_target_size",
    "trust_threshold",
    "youtube_daily_searches",
    "submission_daily_limit",
    "live_minutes_per_point",
];

/// Checks that `key` is known and `value` is an integer within its range.
/// Returns the integer on success.
pub fn validate(key: &str, value: &Value) -> DbResult<i64> {
    if !KNOWN_KEYS.contains(&key) {
        return Err(DbError::InvalidPayload(format!("unknown setting {key}")));
    }
    let n = value
        .as_i64()
        .ok_or_else(|| DbError::InvalidPayload(format!("{key} must be an integer")))?;
    let min = if key == "live_minutes_per_point" {
        1
    } else {
        0
    };
    if n < min {
        return Err(DbError::InvalidPayload(format!(
            "{key} must be at least {min}"
        )));
    }
    Ok(n)
}

/// Reads an integer setting, falling back to `default` when it is missing or not an integer.
pub async fn get_i64(ex: impl PgExecutor<'_>, key: &str, default: i64) -> DbResult<i64> {
    let value: Option<serde_json::Value> =
        sqlx::query_scalar::<_, serde_json::Value>("SELECT value FROM settings WHERE key = $1")
            .bind(key)
            .fetch_optional(ex)
            .await?;
    Ok(value.and_then(|v| v.as_i64()).unwrap_or(default))
}

/// Writes a setting after validating it.
pub async fn set(ex: impl PgExecutor<'_>, key: &str, value: &Value) -> DbResult<()> {
    validate(key, value)?;
    sqlx::query(
        "INSERT INTO settings (key, value) VALUES ($1, $2)
         ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value",
    )
    .bind(key)
    .bind(value.clone())
    .execute(ex)
    .await?;
    Ok(())
}

/// All settings as `(key, value)` pairs, sorted by key.
pub async fn all(ex: impl PgExecutor<'_>) -> DbResult<Vec<(String, Value)>> {
    let rows = sqlx::query_as::<_, (String, Value)>("SELECT key, value FROM settings ORDER BY key")
        .fetch_all(ex)
        .await?;
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn accepts_known_integer() {
        assert_eq!(validate("trust_threshold", &json!(3)).unwrap(), 3);
    }

    #[test]
    fn rejects_unknown_key_and_non_integer() {
        assert!(validate("nope", &json!(1)).is_err());
        assert!(validate("trust_threshold", &json!("3")).is_err());
    }

    #[test]
    fn live_minutes_per_point_must_be_positive() {
        assert!(validate("live_minutes_per_point", &json!(0)).is_err());
        assert!(validate("live_minutes_per_point", &json!(1)).is_ok());
        assert!(validate("trust_threshold", &json!(0)).is_ok());
    }
}
