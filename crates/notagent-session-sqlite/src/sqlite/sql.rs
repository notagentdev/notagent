use rusqlite::ToSql;
use rusqlite::types::{ToSqlOutput, Value};

/// A SQLite parameter value.
#[derive(Debug, Clone, PartialEq)]
pub enum SqlValue {
    Null,
    Integer(i64),
    Real(f64),
    Text(String),
    Blob(Vec<u8>),
}

impl ToSql for SqlValue {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        Ok(match self {
            Self::Null => ToSqlOutput::Owned(Value::Null),
            Self::Integer(value) => ToSqlOutput::Owned(Value::Integer(*value)),
            Self::Real(value) => ToSqlOutput::Owned(Value::Real(*value)),
            Self::Text(value) => {
                ToSqlOutput::Borrowed(rusqlite::types::ValueRef::Text(value.as_bytes()))
            }
            Self::Blob(value) => ToSqlOutput::Borrowed(rusqlite::types::ValueRef::Blob(value)),
        })
    }
}

impl From<i64> for SqlValue {
    fn from(value: i64) -> Self {
        Self::Integer(value)
    }
}

impl From<u64> for SqlValue {
    fn from(value: u64) -> Self {
        Self::Integer(value as i64)
    }
}

impl From<i32> for SqlValue {
    fn from(value: i32) -> Self {
        Self::Integer(i64::from(value))
    }
}

impl From<usize> for SqlValue {
    fn from(value: usize) -> Self {
        Self::Integer(value as i64)
    }
}

impl From<f64> for SqlValue {
    fn from(value: f64) -> Self {
        Self::Real(value)
    }
}

impl From<bool> for SqlValue {
    fn from(value: bool) -> Self {
        Self::Integer(i64::from(value))
    }
}

impl From<String> for SqlValue {
    fn from(value: String) -> Self {
        Self::Text(value)
    }
}

impl From<&str> for SqlValue {
    fn from(value: &str) -> Self {
        Self::Text(value.to_owned())
    }
}

impl<T: Into<SqlValue>> From<Option<T>> for SqlValue {
    fn from(value: Option<T>) -> Self {
        match value {
            Some(value) => value.into(),
            None => Self::Null,
        }
    }
}

/// One piece of a composed query.
pub enum SqlPart {
    Text(String),
    Param(SqlValue),
    Fragment(SqlQuery),
}

/// A parameterized SQLite query.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SqlQuery {
    pub query_text: String,
    pub params: Vec<SqlValue>,
}

impl SqlQuery {
    pub fn new(query_text: impl Into<String>, params: Vec<SqlValue>) -> Self {
        Self {
            query_text: query_text.into(),
            params,
        }
    }

    /// A query without parameters.
    pub fn raw(query_text: impl Into<String>) -> Self {
        Self::new(query_text, Vec::new())
    }

    /// Composes text, parameters and nested fragments in order — the Rust
    /// equivalent of the `sql` tagged template.
    pub fn compose(parts: impl IntoIterator<Item = SqlPart>) -> Self {
        let mut query_text = String::new();
        let mut params = Vec::new();
        for part in parts {
            match part {
                SqlPart::Text(text) => query_text.push_str(&text),
                SqlPart::Param(value) => {
                    query_text.push('?');
                    params.push(value);
                }
                SqlPart::Fragment(fragment) => {
                    query_text.push_str(&fragment.query_text);
                    params.extend(fragment.params);
                }
            }
        }
        Self { query_text, params }
    }
}

/// Joins trusted query fragments while preserving their parameter order.
pub fn join_sql_fragments(fragments: &[SqlQuery], separator: &str) -> SqlQuery {
    let mut query_text = String::new();
    let mut params = Vec::new();
    for (index, fragment) in fragments.iter().enumerate() {
        if index > 0 {
            query_text.push_str(separator);
        }
        query_text.push_str(&fragment.query_text);
        params.extend(fragment.params.iter().cloned());
    }
    SqlQuery { query_text, params }
}

/// Builds a query from alternating text and parameters, mirroring the tagged
/// template: `sql!["SELECT ", param(1), " FROM t"]`.
#[macro_export]
macro_rules! sql {
    ($($part:expr),* $(,)?) => {
        $crate::sqlite::sql::SqlQuery::compose(vec![$($part),*])
    };
}

/// `SqlPart::Text` shorthand.
pub fn text(value: impl Into<String>) -> SqlPart {
    SqlPart::Text(value.into())
}

/// `SqlPart::Param` shorthand.
pub fn param(value: impl Into<SqlValue>) -> SqlPart {
    SqlPart::Param(value.into())
}

/// `SqlPart::Fragment` shorthand.
pub fn fragment(value: SqlQuery) -> SqlPart {
    SqlPart::Fragment(value)
}
