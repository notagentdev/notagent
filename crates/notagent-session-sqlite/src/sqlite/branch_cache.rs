//! Port of `packages/session-backends/sqlite-node/src/sqlite/branch-cache.ts`.

use notagent_agent::uuidv7;

use crate::session_types::{SessionError, SessionErrorCode};
use crate::sql;
use crate::sqlite::sql::{param, text};
use crate::sqlite::storage::branch_entries::{
    copy_branch_entries_through_seq, delete_branch_entries, insert_branch_entries_for_path,
    insert_branch_entry, read_branch_containing_entry,
};
use crate::sqlite::storage::branch_tips::{
    delete_branch_tips, insert_branch_tip, read_branch_tip_branch_id, update_branch_tip,
};
use crate::sqlite::storage::entries::string_column;
use crate::sqlite::types::SqliteDatabase;

pub fn delete_branch_cache(db: &dyn SqliteDatabase, session_id: &str) -> Result<(), SessionError> {
    delete_branch_tips(db, session_id)?;
    delete_branch_entries(db, session_id)
}

pub fn rebuild_branch_cache(db: &dyn SqliteDatabase, session_id: &str) -> Result<(), SessionError> {
    let tips = db.all(&sql![
        text("SELECT leaf.id\n\t\tFROM entries AS leaf\n\t\tWHERE leaf.session_id = "),
        param(session_id),
        text(
            "\n\t\t\tAND NOT EXISTS (\n\t\t\t\tSELECT 1 FROM entries AS child WHERE child.session_id = leaf.session_id AND child.parent_id = leaf.id\n\t\t\t)\n\t\tORDER BY leaf.seq"
        ),
    ])?;
    delete_branch_cache(db, session_id)?;
    for tip in &tips {
        build_cached_branch(db, session_id, &string_column(tip, "id"))?;
    }
    Ok(())
}

pub fn build_cached_branch(
    db: &dyn SqliteDatabase,
    session_id: &str,
    leaf_id: &str,
) -> Result<(), SessionError> {
    db.exec("SAVEPOINT build_branch_cache")?;
    let branch_id = uuidv7();
    let result = insert_branch_entries_for_path(db, session_id, &branch_id, leaf_id)
        .and_then(|()| insert_branch_tip(db, session_id, leaf_id, &branch_id));
    match result {
        Ok(()) => {
            db.exec("RELEASE SAVEPOINT build_branch_cache")?;
            Ok(())
        }
        Err(error) => {
            // Preserve the original build failure.
            let _ = db.exec("ROLLBACK TO SAVEPOINT build_branch_cache");
            let _ = db.exec("RELEASE SAVEPOINT build_branch_cache");
            Err(error)
        }
    }
}

// Mirrors the TS parameter list one to one (CONVENTIONS.md §9).
#[allow(clippy::too_many_arguments)]
fn extend_branch(
    db: &dyn SqliteDatabase,
    session_id: &str,
    branch_id: &str,
    parent_id: &str,
    entry_id: &str,
    entry_seq: i64,
    entry_type: &str,
    custom_type: Option<&str>,
) -> Result<(), SessionError> {
    insert_branch_entry(
        db,
        session_id,
        branch_id,
        entry_id,
        entry_seq,
        entry_type,
        custom_type,
    )?;
    if !update_branch_tip(db, session_id, branch_id, parent_id, entry_id)? {
        return Err(SessionError::new(
            SessionErrorCode::InvalidEntry,
            format!("Branch tip {parent_id} changed during append"),
        ));
    }
    Ok(())
}

// Mirrors the TS parameter list one to one (CONVENTIONS.md §9).
#[allow(clippy::too_many_arguments)]
pub fn append_entry_to_branch_cache(
    db: &dyn SqliteDatabase,
    session_id: &str,
    entry_id: &str,
    entry_seq: i64,
    entry_type: &str,
    custom_type: Option<&str>,
    parent_id: Option<&str>,
) -> Result<(), SessionError> {
    let Some(parent_id) = parent_id else {
        let branch_id = uuidv7();
        insert_branch_entry(
            db,
            session_id,
            &branch_id,
            entry_id,
            entry_seq,
            entry_type,
            custom_type,
        )?;
        return insert_branch_tip(db, session_id, entry_id, &branch_id);
    };

    if let Some(tip_branch_id) = read_branch_tip_branch_id(db, session_id, parent_id)? {
        return extend_branch(
            db,
            session_id,
            &tip_branch_id,
            parent_id,
            entry_id,
            entry_seq,
            entry_type,
            custom_type,
        );
    }

    let Some((source_branch_id, source_entry_seq)) =
        read_branch_containing_entry(db, session_id, parent_id)?
    else {
        return Err(SessionError::new(
            SessionErrorCode::InvalidEntry,
            format!("Branch cache has no branch containing parent entry {parent_id}"),
        ));
    };

    let branch_id = uuidv7();
    copy_branch_entries_through_seq(
        db,
        session_id,
        &branch_id,
        &source_branch_id,
        source_entry_seq,
    )?;
    insert_branch_entry(
        db,
        session_id,
        &branch_id,
        entry_id,
        entry_seq,
        entry_type,
        custom_type,
    )?;
    insert_branch_tip(db, session_id, entry_id, &branch_id)
}
