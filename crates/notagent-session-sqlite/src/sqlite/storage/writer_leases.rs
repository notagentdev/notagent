use crate::session_types::SessionError;
use crate::sql;
use crate::sqlite::sql::{param, text};
use crate::sqlite::types::SqliteDatabase;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriterLease {
    pub owner_id: String,
    pub fence: i64,
    pub expires_at_ms: i64,
}

pub fn acquire_writer_lease(
    db: &dyn SqliteDatabase,
    session_id: &str,
    owner_id: &str,
    now: i64,
    expires_at_ms: i64,
) -> Result<Option<WriterLease>, SessionError> {
    let row = db.get(&sql![
        text("INSERT INTO writer_leases (session_id, owner_id, fence, expires_at_ms)\n\t\tVALUES ("),
        param(session_id),
        text(", "),
        param(owner_id),
        text(", 1, "),
        param(expires_at_ms),
        text(
            ")\n\t\tON CONFLICT(session_id) DO UPDATE SET\n\t\t\towner_id = excluded.owner_id,\n\t\t\tfence = writer_leases.fence + 1,\n\t\t\texpires_at_ms = excluded.expires_at_ms\n\t\tWHERE writer_leases.expires_at_ms <= "
        ),
        param(now),
        text("\n\t\tRETURNING owner_id, fence, expires_at_ms"),
    ])?;
    Ok(row.map(|row| WriterLease {
        owner_id: row
            .get("owner_id")
            .and_then(|value| value.as_str())
            .unwrap_or_default()
            .to_owned(),
        fence: row
            .get("fence")
            .and_then(serde_json::Value::as_i64)
            .unwrap_or_default(),
        expires_at_ms: row
            .get("expires_at_ms")
            .and_then(serde_json::Value::as_i64)
            .unwrap_or_default(),
    }))
}

pub fn renew_writer_lease(
    db: &dyn SqliteDatabase,
    session_id: &str,
    lease: &mut WriterLease,
    now: i64,
    expires_at_ms: i64,
) -> Result<bool, SessionError> {
    let result = db.run(&sql![
        text("UPDATE writer_leases\n\t\tSET expires_at_ms = "),
        param(expires_at_ms),
        text("\n\t\tWHERE session_id = "),
        param(session_id),
        text("\n\t\t\tAND owner_id = "),
        param(lease.owner_id.as_str()),
        text("\n\t\t\tAND fence = "),
        param(lease.fence),
        text("\n\t\t\tAND expires_at_ms > "),
        param(now),
    ])?;
    if result.changes == 1 {
        lease.expires_at_ms = expires_at_ms;
    }
    Ok(result.changes == 1)
}

pub fn release_writer_lease(
    db: &dyn SqliteDatabase,
    session_id: &str,
    lease: &WriterLease,
) -> Result<(), SessionError> {
    db.run(&sql![
        text("DELETE FROM writer_leases\n\t\tWHERE session_id = "),
        param(session_id),
        text(" AND owner_id = "),
        param(lease.owner_id.as_str()),
        text(" AND fence = "),
        param(lease.fence),
    ])?;
    Ok(())
}

pub fn delete_writer_lease(db: &dyn SqliteDatabase, session_id: &str) -> Result<(), SessionError> {
    db.run(&sql![
        text("DELETE FROM writer_leases WHERE session_id = "),
        param(session_id)
    ])?;
    Ok(())
}
