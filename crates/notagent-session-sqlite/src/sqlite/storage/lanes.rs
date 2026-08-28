use crate::session_types::{SessionError, SessionErrorCode};
use crate::sql;
use crate::sqlite::sql::{SqlQuery, fragment, param, text};
use crate::sqlite::types::{Row, SqliteDatabase};

use super::entries::{integer_column, string_column};

#[derive(Debug, Clone, PartialEq)]
pub struct LaneRow {
    pub session_id: String,
    pub lane: String,
    pub leaf_id: Option<String>,
    pub open_operation_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LaneMoveRow {
    pub session_id: String,
    pub seq: i64,
    pub lane: String,
    pub leaf_id: Option<String>,
}

fn lane_row(row: &Row) -> LaneRow {
    LaneRow {
        session_id: string_column(row, "session_id"),
        lane: string_column(row, "lane"),
        leaf_id: row
            .get("leaf_id")
            .and_then(|value| value.as_str())
            .map(str::to_owned),
        open_operation_id: row
            .get("open_operation_id")
            .and_then(|value| value.as_str())
            .map(str::to_owned),
    }
}

pub fn create_initial_lane(
    db: &dyn SqliteDatabase,
    session_id: &str,
    lane: &str,
    leaf_id: Option<&str>,
) -> Result<(), SessionError> {
    db.run(&sql![
        text("INSERT INTO lanes (session_id, lane, leaf_id, open_operation_id)\n\t\tVALUES ("),
        param(session_id),
        text(", "),
        param(lane),
        text(", "),
        param(leaf_id.map(str::to_owned)),
        text(", NULL)"),
    ])?;
    Ok(())
}

pub fn read_lanes(db: &dyn SqliteDatabase, session_id: &str) -> Result<Vec<LaneRow>, SessionError> {
    let rows = db.all(&sql![
        text(
            "SELECT\n\t\t\tl.session_id,\n\t\t\tl.lane,\n\t\t\tl.leaf_id,\n\t\t\tl.open_operation_id,\n\t\t\t(l.leaf_id IS NULL OR EXISTS (\n\t\t\t\tSELECT 1 FROM entries AS e WHERE e.session_id = l.session_id AND e.id = l.leaf_id\n\t\t\t)) AS leaf_exists\n\t\tFROM lanes AS l\n\t\tWHERE l.session_id = "
        ),
        param(session_id),
        text("\n\t\tORDER BY l.lane"),
    ])?;
    for row in &rows {
        if integer_column(row, "leaf_exists") == 0 {
            let lane = string_column(row, "lane");
            let leaf_id = row
                .get("leaf_id")
                .and_then(|value| value.as_str())
                .unwrap_or("null")
                .to_owned();
            return Err(SessionError::storage(format!(
                "Lane {lane} points at missing entry {leaf_id}"
            )));
        }
    }
    Ok(rows.iter().map(lane_row).collect())
}

pub fn read_lane(
    db: &dyn SqliteDatabase,
    session_id: &str,
    lane: &str,
) -> Result<Option<LaneRow>, SessionError> {
    let row = db.get(&sql![
        text("SELECT session_id, lane, leaf_id, open_operation_id\n\t\tFROM lanes\n\t\tWHERE session_id = "),
        param(session_id),
        text(" AND lane = "),
        param(lane),
    ])?;
    Ok(row.as_ref().map(lane_row))
}

pub fn read_lane_head(
    db: &dyn SqliteDatabase,
    session_id: &str,
    lane: &str,
) -> Result<Option<String>, SessionError> {
    let row = db.get(&sql![
        text(
            "SELECT\n\t\t\tl.leaf_id,\n\t\t\t(l.leaf_id IS NULL OR EXISTS (\n\t\t\t\tSELECT 1 FROM entries AS e WHERE e.session_id = l.session_id AND e.id = l.leaf_id\n\t\t\t)) AS leaf_exists\n\t\tFROM lanes AS l\n\t\tWHERE l.session_id = "
        ),
        param(session_id),
        text(" AND l.lane = "),
        param(lane),
    ])?;
    let row = row.ok_or_else(|| {
        SessionError::new(
            SessionErrorCode::InvalidLane,
            format!("Lane not found: {lane}"),
        )
    })?;
    if integer_column(&row, "leaf_exists") == 0 {
        let leaf_id = row
            .get("leaf_id")
            .and_then(|value| value.as_str())
            .unwrap_or("null")
            .to_owned();
        return Err(SessionError::storage(format!("Entry {leaf_id} not found")));
    }
    Ok(row
        .get("leaf_id")
        .and_then(|value| value.as_str())
        .map(str::to_owned))
}

pub fn create_lane(
    db: &dyn SqliteDatabase,
    session_id: &str,
    seq: i64,
    lane: &str,
    leaf_id: Option<&str>,
) -> Result<(), SessionError> {
    create_initial_lane(db, session_id, lane, leaf_id)?;
    append_lane_move(db, session_id, seq, lane, leaf_id)
}

pub fn move_lane(
    db: &dyn SqliteDatabase,
    session_id: &str,
    seq: i64,
    lane: &str,
    leaf_id: Option<&str>,
) -> Result<(), SessionError> {
    set_lane_leaf(db, session_id, lane, leaf_id)?;
    append_lane_move(db, session_id, seq, lane, leaf_id)
}

pub fn set_lane_leaf(
    db: &dyn SqliteDatabase,
    session_id: &str,
    lane: &str,
    leaf_id: Option<&str>,
) -> Result<(), SessionError> {
    let result = db.run(&sql![
        text("UPDATE lanes SET leaf_id = "),
        param(leaf_id.map(str::to_owned)),
        text(" WHERE session_id = "),
        param(session_id),
        text(" AND lane = "),
        param(lane),
    ])?;
    if result.changes != 1 {
        return Err(SessionError::new(
            SessionErrorCode::InvalidLane,
            format!("Lane not found: {lane}"),
        ));
    }
    Ok(())
}

pub fn start_lane_operation(
    db: &dyn SqliteDatabase,
    session_id: &str,
    lane: &str,
    run_id: &str,
) -> Result<(), SessionError> {
    let result = db.run(&sql![
        text("UPDATE lanes SET open_operation_id = "),
        param(run_id),
        text("\n\t\tWHERE session_id = "),
        param(session_id),
        text(" AND lane = "),
        param(lane),
        text(" AND open_operation_id IS NULL"),
    ])?;
    if result.changes == 1 {
        return Ok(());
    }
    let current = read_lane(db, session_id, lane)?.ok_or_else(|| {
        SessionError::new(
            SessionErrorCode::InvalidLane,
            format!("Lane not found: {lane}"),
        )
    })?;
    Err(SessionError::storage(format!(
        "Lane {lane} already has an open operation {}",
        current
            .open_operation_id
            .unwrap_or_else(|| "null".to_owned())
    )))
}

pub fn finish_lane_operation(
    db: &dyn SqliteDatabase,
    session_id: &str,
    lane: &str,
    run_id: &str,
) -> Result<(), SessionError> {
    db.run(&sql![
        text("UPDATE lanes SET open_operation_id = NULL\n\t\tWHERE session_id = "),
        param(session_id),
        text(" AND lane = "),
        param(lane),
        text(" AND open_operation_id = "),
        param(run_id),
    ])?;
    Ok(())
}

pub fn read_lane_move_rows(
    db: &dyn SqliteDatabase,
    session_id: &str,
    after_seq: Option<i64>,
    limit: Option<i64>,
) -> Result<Vec<LaneMoveRow>, SessionError> {
    let after = match after_seq {
        None => SqlQuery::default(),
        Some(after_seq) => sql![text(" AND seq > "), param(after_seq)],
    };
    let limit = match limit {
        None => SqlQuery::default(),
        Some(limit) => sql![text(" LIMIT "), param(limit)],
    };
    let rows = db.all(&sql![
        text("SELECT session_id, seq, lane, leaf_id\n\t\tFROM lane_moves\n\t\tWHERE session_id = "),
        param(session_id),
        fragment(after),
        text("\n\t\tORDER BY seq"),
        fragment(limit),
    ])?;
    Ok(rows
        .iter()
        .map(|row| LaneMoveRow {
            session_id: string_column(row, "session_id"),
            seq: integer_column(row, "seq"),
            lane: string_column(row, "lane"),
            leaf_id: row
                .get("leaf_id")
                .and_then(|value| value.as_str())
                .map(str::to_owned),
        })
        .collect())
}

pub fn delete_lane_rows(db: &dyn SqliteDatabase, session_id: &str) -> Result<(), SessionError> {
    db.run(&sql![
        text("DELETE FROM lane_moves WHERE session_id = "),
        param(session_id)
    ])?;
    db.run(&sql![
        text("DELETE FROM lanes WHERE session_id = "),
        param(session_id)
    ])?;
    Ok(())
}

fn append_lane_move(
    db: &dyn SqliteDatabase,
    session_id: &str,
    seq: i64,
    lane: &str,
    leaf_id: Option<&str>,
) -> Result<(), SessionError> {
    db.run(&sql![
        text("INSERT INTO lane_moves (session_id, seq, lane, leaf_id) VALUES ("),
        param(session_id),
        text(", "),
        param(seq),
        text(", "),
        param(lane),
        text(", "),
        param(leaf_id.map(str::to_owned)),
        text(")"),
    ])?;
    Ok(())
}
