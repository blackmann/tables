//! What the app knows about a database, independent of which engine serves it.
//!
//! Drivers are blocking and run on background threads behind a mutex; the UI
//! only ever sees the plain data defined here. Writes from the grid are turned
//! into literal SQL by [`Change::to_sql`] and that same text is what runs, so
//! the review dialog shows exactly what will be executed.
pub mod config;
pub mod postgres;
pub mod sqlite;

use anyhow::Result;
use std::fmt::Write as _;
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Engine {
    Postgres,
    Sqlite,
}

impl Engine {
    pub fn label(self) -> &'static str {
        match self {
            Engine::Postgres => "PostgreSQL",
            Engine::Sqlite => "SQLite",
        }
    }
    /// The mark on the engine's badge.
    pub fn glyph(self) -> crate::icons::Glyph {
        match self {
            Engine::Postgres => crate::icons::Glyph::Elephant,
            Engine::Sqlite => crate::icons::Glyph::Feather,
        }
    }
    /// The badge's gradient, top to bottom.
    pub fn badge_colors(self) -> (u32, u32) {
        match self {
            Engine::Postgres => (0x6aa5ff, 0x3a6ee8),
            Engine::Sqlite => (0x4fd6ff, 0x109ee0),
        }
    }
    pub fn quote(self, ident: &str) -> String {
        format!("\"{}\"", ident.replace('"', "\"\""))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Null,
    /// The column's default, for a row that hasn't been inserted yet.
    Default,
    Bool(bool),
    Int(i64),
    Float(f64),
    /// Exact numbers (numeric/decimal) kept as text so nothing is rounded.
    Number(String),
    Text(String),
    Json(serde_json::Value),
    Bytes(Vec<u8>),
}

impl Value {
    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }
    /// How the value reads in a cell or a field.
    pub fn display(&self) -> String {
        match self {
            Value::Null => "NULL".into(),
            Value::Default => "DEFAULT".into(),
            Value::Bool(b) => b.to_string(),
            Value::Int(i) => i.to_string(),
            Value::Float(f) => f.to_string(),
            Value::Number(s) | Value::Text(s) => s.clone(),
            Value::Json(j) => j.to_string(),
            Value::Bytes(b) => {
                let mut s = String::from("\\x");
                for byte in b.iter().take(64) {
                    let _ = write!(s, "{byte:02x}");
                }
                if b.len() > 64 {
                    s.push('…');
                }
                s
            }
        }
    }
    /// The text put in an editor for this value; NULL and DEFAULT edit as empty.
    pub fn edit_text(&self) -> String {
        match self {
            Value::Null | Value::Default => String::new(),
            Value::Json(j) => serde_json::to_string_pretty(j).unwrap_or_default(),
            other => other.display(),
        }
    }
    /// A SQL literal. Everything but NULL, DEFAULT and booleans is quoted: both
    /// engines coerce a quoted literal to the column's type, which keeps this
    /// independent of what the column holds.
    pub fn literal(&self) -> String {
        match self {
            Value::Null => "NULL".into(),
            Value::Default => "DEFAULT".into(),
            Value::Bool(b) => if *b { "TRUE" } else { "FALSE" }.into(),
            Value::Int(i) => i.to_string(),
            Value::Float(f) => f.to_string(),
            Value::Bytes(b) => {
                let mut s = String::from("'\\x");
                for byte in b {
                    let _ = write!(s, "{byte:02x}");
                }
                s + "'"
            }
            other => quote_text(&other.display()),
        }
    }
    /// Read what someone typed into an editor for a column of `kind`.
    pub fn parse(text: &str, kind: Kind, was_null: bool) -> Value {
        if text == "NULL" || (text.is_empty() && was_null) {
            return Value::Null;
        }
        match kind {
            Kind::Bool => Value::Bool(matches!(text.trim().to_ascii_lowercase().as_str(), "t" | "true" | "1" | "yes" | "y" | "on")),
            Kind::Int => text.trim().parse().map(Value::Int).unwrap_or_else(|_| Value::Text(text.into())),
            Kind::Float => text.trim().parse().map(Value::Float).unwrap_or_else(|_| Value::Text(text.into())),
            Kind::Number => Value::Number(text.trim().into()),
            Kind::Json => serde_json::from_str(text).map(Value::Json).unwrap_or_else(|_| Value::Text(text.into())),
            _ => Value::Text(text.into()),
        }
    }
    /// Order two values for client-side sorting: NULLs last, numbers by value.
    #[allow(dead_code)]
    #[allow(dead_code)]
    pub fn cmp_for_sort(&self, other: &Value) -> std::cmp::Ordering {
        use std::cmp::Ordering::*;
        match (self, other) {
            (Value::Null, Value::Null) => Equal,
            (Value::Null, _) => Greater,
            (_, Value::Null) => Less,
            _ => match (self.as_f64(), other.as_f64()) {
                (Some(a), Some(b)) => a.partial_cmp(&b).unwrap_or(Equal),
                _ => self.display().cmp(&other.display()),
            },
        }
    }
    #[allow(dead_code)]
    fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Int(i) => Some(*i as f64),
            Value::Float(f) => Some(*f),
            Value::Number(s) => s.parse().ok(),
            _ => None,
        }
    }
}

pub fn quote_text(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// How a column's values are shown and edited, from its declared type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Bool,
    Int,
    Float,
    Number,
    Text,
    Json,
    Time,
    Uuid,
    Bytes,
    Array,
    Enum,
    Other,
}

impl Kind {
    pub fn from_type(t: &str) -> Kind {
        let t = t.to_ascii_lowercase();
        let base = t.split('(').next().unwrap_or("").trim();
        if t.ends_with("[]") || t.starts_with('_') {
            return Kind::Array;
        }
        match base {
            "bool" | "boolean" => Kind::Bool,
            "int" | "int2" | "int4" | "int8" | "integer" | "smallint" | "bigint" | "serial" | "bigserial" | "smallserial" | "oid" | "tinyint" | "mediumint" => Kind::Int,
            "real" | "float" | "float4" | "float8" | "double" | "double precision" => Kind::Float,
            "numeric" | "decimal" | "money" => Kind::Number,
            "json" | "jsonb" => Kind::Json,
            "uuid" => Kind::Uuid,
            "bytea" | "blob" => Kind::Bytes,
            "text" | "varchar" | "character varying" | "char" | "character" | "bpchar" | "name" | "citext" | "string" | "clob" => Kind::Text,
            _ if base.starts_with("timestamp") || base == "date" || base.starts_with("time") || base == "datetime" || base == "interval" => Kind::Time,
            _ if base.starts_with("varchar") || base.starts_with("nvarchar") || base.contains("char") => Kind::Text,
            _ if base.contains("int") => Kind::Int,
            _ => Kind::Other,
        }
    }
    pub fn right_aligned(self) -> bool {
        matches!(self, Kind::Int | Kind::Float | Kind::Number)
    }
    pub fn mono(self) -> bool {
        matches!(self, Kind::Int | Kind::Float | Kind::Number | Kind::Json | Kind::Time | Kind::Uuid | Kind::Bytes)
    }
}

#[derive(Clone, Debug, Default)]
pub struct ColumnInfo {
    pub name: String,
    pub data_type: String,
    pub nullable: bool,
    pub default: Option<String>,
    pub primary_key: bool,
    pub unique: bool,
    /// The table and column this one references.
    pub foreign: Option<(String, String)>,
    /// Allowed values, for enum types.
    pub enum_values: Vec<String>,
    pub comment: Option<String>,
}

impl ColumnInfo {
    pub fn kind(&self) -> Kind {
        if !self.enum_values.is_empty() { Kind::Enum } else { Kind::from_type(&self.data_type) }
    }
}

#[derive(Clone, Debug)]
pub struct IndexInfo {
    pub name: String,
    pub columns: Vec<String>,
    pub unique: bool,
    pub primary: bool,
    pub method: String,
    pub size: Option<String>,
    pub scans: Option<i64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RelationKind {
    Table,
    View,
}

#[derive(Clone, Debug)]
pub struct Relation {
    pub name: String,
    pub kind: RelationKind,
    /// Rows, as the engine estimates them; `None` where it can't say cheaply.
    pub rows: Option<i64>,
}

#[derive(Clone, Debug, Default)]
pub struct TableDetail {
    pub columns: Vec<ColumnInfo>,
    pub indexes: Vec<IndexInfo>,
    /// Tables whose columns point at this one: (table, column).
    pub referenced_by: Vec<(String, String)>,
    pub ddl: String,
    pub size: Option<String>,
}

#[derive(Clone, Debug)]
pub struct ResultColumn {
    pub name: String,
    pub type_name: String,
}

#[derive(Clone, Debug, Default)]
pub struct Page {
    pub columns: Vec<ResultColumn>,
    pub rows: Vec<Vec<Value>>,
    pub elapsed: Duration,
}

#[derive(Clone, Debug)]
pub enum Outcome {
    Rows(Page),
    /// A statement that changed things rather than returning rows.
    Done { command: String, rows: Option<u64>, elapsed: Duration },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    Eq,
    Ne,
    Lt,
    Gt,
    Lte,
    Gte,
    In,
    NotIn,
    IsNull,
    NotNull,
    Between,
    NotBetween,
    Like,
    ILike,
    Contains,
    NotContains,
    ContainsCi,
    NotContainsCi,
}

impl Op {
    /// Operators in menu order; groups are separated in the picker.
    pub const GROUPS: &[&[Op]] = &[
        &[Op::Eq, Op::Ne, Op::Lt, Op::Gt, Op::Lte, Op::Gte],
        &[Op::In, Op::NotIn],
        &[Op::IsNull, Op::NotNull],
        &[Op::Between, Op::NotBetween],
        &[Op::Like, Op::ILike],
        &[Op::Contains, Op::NotContains],
        &[Op::ContainsCi, Op::NotContainsCi],
    ];
    pub fn label(self) -> &'static str {
        match self {
            Op::Eq => "=",
            Op::Ne => "<>",
            Op::Lt => "<",
            Op::Gt => ">",
            Op::Lte => "<=",
            Op::Gte => ">=",
            Op::In => "IN",
            Op::NotIn => "NOT IN",
            Op::IsNull => "IS NULL",
            Op::NotNull => "IS NOT NULL",
            Op::Between => "BETWEEN",
            Op::NotBetween => "NOT BETWEEN",
            Op::Like => "LIKE",
            Op::ILike => "ILIKE",
            Op::Contains => "Contains",
            Op::NotContains => "Not contains",
            Op::ContainsCi => "Contains - Case insensitive",
            Op::NotContainsCi => "Not contains - Case insensitive",
        }
    }
    pub fn takes_value(self) -> bool {
        !matches!(self, Op::IsNull | Op::NotNull)
    }
    /// What to type, shown in an empty value field.
    pub fn hint(self) -> &'static str {
        match self {
            Op::In | Op::NotIn => "a, b, c",
            Op::Between | Op::NotBetween => "low, high",
            Op::Like | Op::ILike => "pattern with % and _",
            _ => "EMPTY",
        }
    }
}

/// What a filter row looks at.
#[derive(Clone, Debug, PartialEq)]
pub enum Target {
    Column(String),
    /// Matches when any column does.
    AnyColumn,
    /// The value is a SQL condition, used as is.
    Raw,
}

#[derive(Clone, Debug)]
pub struct Filter {
    pub target: Target,
    pub op: Op,
    pub value: String,
}

impl Filter {
    pub fn column(column: impl Into<String>, op: Op, value: impl Into<String>) -> Self {
        Filter { target: Target::Column(column.into()), op, value: value.into() }
    }
    /// Whether the filter says anything yet.
    pub fn is_complete(&self) -> bool {
        match self.target {
            Target::Raw => !self.value.trim().is_empty(),
            _ => !self.op.takes_value() || !self.value.is_empty(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Sort {
    pub column: String,
    pub descending: bool,
}

/// One row's worth of pending edits, ready to become a statement.
#[derive(Clone, Debug)]
pub enum Change {
    Insert { values: Vec<(String, Value)> },
    Update { key: Vec<(String, Value)>, set: Vec<(String, Value)> },
    Delete { key: Vec<(String, Value)> },
}

impl Change {
    pub fn to_sql(&self, engine: Engine, table: &str) -> String {
        let q = |s: &str| engine.quote(s);
        let cond = |key: &[(String, Value)]| {
            key.iter()
                .map(|(c, v)| if v.is_null() { format!("{} IS NULL", q(c)) } else { format!("{} = {}", q(c), v.literal()) })
                .collect::<Vec<_>>()
                .join(" AND ")
        };
        match self {
            Change::Insert { values } => {
                let given: Vec<_> = values.iter().filter(|(_, v)| *v != Value::Default).collect();
                if given.is_empty() {
                    return format!("INSERT INTO {table} DEFAULT VALUES;");
                }
                format!(
                    "INSERT INTO {table} ({})\nVALUES ({});",
                    given.iter().map(|(c, _)| q(c)).collect::<Vec<_>>().join(", "),
                    given.iter().map(|(_, v)| v.literal()).collect::<Vec<_>>().join(", ")
                )
            }
            Change::Update { key, set } => format!(
                "UPDATE {table}\nSET {}\nWHERE {};",
                set.iter().map(|(c, v)| format!("{} = {}", q(c), v.literal())).collect::<Vec<_>>().join(",\n    "),
                cond(key)
            ),
            Change::Delete { key } => format!("DELETE FROM {table} WHERE {};", cond(key)),
        }
    }
}

/// `schema.table`, quoted for `engine`; SQLite's main schema is left off.
pub fn qualified(engine: Engine, schema: &str, table: &str) -> String {
    match engine {
        Engine::Sqlite if schema.is_empty() || schema == "main" => engine.quote(table),
        _ => format!("{}.{}", engine.quote(schema), engine.quote(table)),
    }
}

/// One column compared by `op` against `value`.
fn condition(engine: Engine, column: &str, op: Op, value: &str) -> String {
    let c = engine.quote(column);
    let text = match engine {
        Engine::Postgres => format!("{c}::text"),
        Engine::Sqlite => format!("CAST({c} AS TEXT)"),
    };
    let v = quote_text(value);
    let list = || value.split(',').map(|x| quote_text(x.trim())).collect::<Vec<_>>().join(", ");
    let range = || {
        let lower = value.to_ascii_lowercase();
        let (a, b) = match lower.find(" and ") {
            Some(i) => (&value[..i], &value[i + 5..]),
            None => value.split_once(',').unwrap_or((value, "")),
        };
        format!("{} AND {}", quote_text(a.trim()), quote_text(b.trim()))
    };
    let pattern = quote_text(&format!("%{value}%"));
    // SQLite's LIKE ignores case already; instr() is its case-sensitive search.
    let (ci_like, cs_contains, cs_not) = match engine {
        Engine::Postgres => ("ILIKE", format!("{text} LIKE {pattern}"), format!("{text} NOT LIKE {pattern}")),
        Engine::Sqlite => ("LIKE", format!("instr({text}, {v}) > 0"), format!("instr({text}, {v}) = 0")),
    };
    match op {
        Op::Eq => format!("{c} = {v}"),
        Op::Ne => format!("{c} <> {v}"),
        Op::Lt => format!("{c} < {v}"),
        Op::Gt => format!("{c} > {v}"),
        Op::Lte => format!("{c} <= {v}"),
        Op::Gte => format!("{c} >= {v}"),
        Op::In => format!("{c} IN ({})", list()),
        Op::NotIn => format!("{c} NOT IN ({})", list()),
        Op::IsNull => format!("{c} IS NULL"),
        Op::NotNull => format!("{c} IS NOT NULL"),
        Op::Between => format!("{c} BETWEEN {}", range()),
        Op::NotBetween => format!("{c} NOT BETWEEN {}", range()),
        Op::Like => format!("{text} LIKE {v}"),
        Op::ILike => format!("{text} {ci_like} {v}"),
        Op::Contains => cs_contains,
        Op::NotContains => cs_not,
        Op::ContainsCi => format!("{text} {ci_like} {pattern}"),
        Op::NotContainsCi => format!("{text} NOT {ci_like} {pattern}"),
    }
}

/// The WHERE for a set of filters; `columns` is what "any column" spans.
pub fn where_clause(engine: Engine, filters: &[Filter], columns: &[String]) -> String {
    let parts: Vec<String> = filters
        .iter()
        .filter(|f| f.is_complete())
        .filter_map(|f| match &f.target {
            Target::Raw => Some(format!("({})", f.value.trim())),
            Target::Column(c) => Some(condition(engine, c, f.op, &f.value)),
            Target::AnyColumn if columns.is_empty() => None,
            Target::AnyColumn => {
                // Negative operators must hold for every column; the rest for any.
                let joiner = if matches!(f.op, Op::Ne | Op::NotIn | Op::NotNull | Op::NotBetween | Op::NotContains | Op::NotContainsCi) { " AND " } else { " OR " };
                Some(format!("({})", columns.iter().map(|c| condition(engine, c, f.op, &f.value)).collect::<Vec<_>>().join(joiner)))
            }
        })
        .collect();
    if parts.is_empty() { String::new() } else { format!("WHERE {}", parts.join(" AND ")) }
}

/// WHERE for the grid: the filters, plus the toolbar's search across every column.
pub fn grid_where(engine: Engine, filters: &[Filter], columns: &[String], search: &str) -> String {
    let mut all = filters.to_vec();
    if !search.trim().is_empty() {
        all.push(Filter { target: Target::AnyColumn, op: Op::ContainsCi, value: search.trim().to_string() });
    }
    where_clause(engine, &all, columns)
}

pub fn select_page(engine: Engine, schema: &str, table: &str, where_sql: &str, sort: Option<&Sort>, limit: usize, offset: usize) -> String {
    let mut sql = format!("SELECT * FROM {}", qualified(engine, schema, table));
    if !where_sql.is_empty() {
        sql += " ";
        sql += where_sql;
    }
    if let Some(s) = sort {
        let _ = write!(sql, " ORDER BY {} {}", engine.quote(&s.column), if s.descending { "DESC" } else { "ASC" });
    }
    let _ = write!(sql, " LIMIT {limit} OFFSET {offset}");
    sql
}

pub fn count_sql(engine: Engine, schema: &str, table: &str, where_sql: &str) -> String {
    format!("SELECT count(*) FROM {} {where_sql}", qualified(engine, schema, table))
}

/// A node of a query plan, flattened with its depth for drawing as a tree.
#[derive(Clone, Debug, Default)]
pub struct PlanNode {
    pub depth: usize,
    pub operation: String,
    pub relation: Option<String>,
    pub detail: String,
    pub rows: Option<f64>,
    /// Time spent in this node alone, when the plan was run (EXPLAIN ANALYZE).
    pub self_ms: Option<f64>,
    pub removed_by_filter: Option<f64>,
}

#[derive(Clone, Debug, Default)]
pub struct Plan {
    pub nodes: Vec<PlanNode>,
    pub summary: String,
    /// A suggestion drawn from the plan, such as an index that would help.
    pub advice: Option<(String, String)>,
}

/// Everything the UI asks of a database. Calls block; run them off the UI thread.
pub trait Driver: Send {
    #[allow(dead_code)]
    fn engine(&self) -> Engine;
    fn version(&mut self) -> Result<String>;
    fn schemas(&mut self) -> Result<Vec<String>>;
    fn default_schema(&self) -> String;
    fn relations(&mut self, schema: &str) -> Result<Vec<Relation>>;
    fn functions(&mut self, schema: &str) -> Result<Vec<(String, String)>>;
    fn detail(&mut self, schema: &str, table: &str) -> Result<TableDetail>;
    /// Every column in the schema as (table, column, type), for completion.
    fn all_columns(&mut self, schema: &str) -> Result<Vec<(String, String, String)>>;
    fn query(&mut self, sql: &str) -> Result<Page>;
    /// Run a script, statement by statement, stopping at the first error.
    fn execute(&mut self, sql: &str) -> Result<Vec<(String, Outcome)>, (Vec<(String, Outcome)>, String, anyhow::Error)>;
    /// Run statements in one transaction; nothing is kept if any fails.
    fn apply(&mut self, statements: &[String]) -> Result<()>;
    fn explain(&mut self, sql: &str) -> Result<Plan>;
    /// Whether the server has dropped the connection. Drivers that reconnect
    /// do so on their next piece of work.
    fn is_closed(&self) -> bool {
        false
    }
    /// Check with the server that the connection still works.
    fn ping(&mut self) -> bool {
        true
    }
    /// Connect again now, dropped or not.
    fn reconnect(&mut self) -> Result<()> {
        Ok(())
    }
}

/// Split a script into statements on semicolons outside quotes and comments.
pub fn split_statements(sql: &str) -> Vec<String> {
    statement_ranges(sql).into_iter().map(|r| sql[r].trim().to_string()).collect()
}

/// Byte ranges of each statement in a script (without its semicolon), skipping
/// ones that are only whitespace and comments.
pub fn statement_ranges(sql: &str) -> Vec<std::ops::Range<usize>> {
    let chars: Vec<(usize, char)> = sql.char_indices().collect();
    let mut out = Vec::new();
    let mut start = 0;
    let (mut single, mut double, mut line_comment, mut block_comment) = (false, false, false, false);
    let mut dollar: Option<String> = None;
    let mut i = 0;
    let at = |i: usize| chars.get(i).map(|c| c.1);
    let push = |s: usize, e: usize, out: &mut Vec<std::ops::Range<usize>>| {
        if has_code(&sql[s..e]) {
            let text = &sql[s..e];
            let lead = text.len() - text.trim_start().len();
            let trail = text.len() - text.trim_end().len();
            out.push(s + lead..e - trail);
        }
    };
    while i < chars.len() {
        let (pos, c) = chars[i];
        let next = at(i + 1);
        if line_comment {
            if c == '\n' {
                line_comment = false;
            }
        } else if block_comment {
            if c == '*' && next == Some('/') {
                i += 1;
                block_comment = false;
            }
        } else if let Some(tag) = &dollar {
            if c == '$' && sql[pos..].starts_with(tag.as_str()) {
                i += tag.chars().count() - 1;
                dollar = None;
            }
        } else if single {
            if c == '\'' {
                single = false;
            }
        } else if double {
            if c == '"' {
                double = false;
            }
        } else {
            match c {
                '\'' => single = true,
                '"' => double = true,
                '-' if next == Some('-') => line_comment = true,
                '/' if next == Some('*') => block_comment = true,
                '$' => {
                    let mut j = i + 1;
                    while j < chars.len() && (chars[j].1.is_alphanumeric() || chars[j].1 == '_') {
                        j += 1;
                    }
                    if j < chars.len() && chars[j].1 == '$' {
                        dollar = Some(sql[pos..=chars[j].0].to_string());
                        i = j;
                    }
                }
                ';' => {
                    push(start, pos, &mut out);
                    start = pos + 1;
                }
                _ => {}
            }
        }
        i += 1;
    }
    push(start, sql.len(), &mut out);
    out
}

/// The statement the cursor is in, or the one just before it.
pub fn statement_at(sql: &str, offset: usize) -> Option<std::ops::Range<usize>> {
    let ranges = statement_ranges(sql);
    ranges
        .iter()
        .find(|r| r.start <= offset && offset <= r.end + 1)
        .or_else(|| ranges.iter().rev().find(|r| r.end <= offset))
        .or(ranges.first())
        .cloned()
}

/// Whether some of `s` is SQL rather than whitespace and comments.
pub fn has_code(s: &str) -> bool {
    strip_comments(s).trim().trim_end_matches(';').trim().len() > 0
}

pub fn strip_comments(s: &str) -> String {
    let mut out = String::new();
    let mut in_block = false;
    for line in s.lines() {
        let mut l = line;
        if in_block {
            match l.find("*/") {
                Some(p) => {
                    l = &l[p + 2..];
                    in_block = false;
                }
                None => continue,
            }
        }
        let mut kept = String::new();
        let mut rest = l;
        loop {
            let dash = rest.find("--");
            let block = rest.find("/*");
            match (dash, block) {
                (Some(d), b) if b.is_none_or(|b| d < b) => {
                    kept += &rest[..d];
                    break;
                }
                (_, Some(b)) => {
                    kept += &rest[..b];
                    match rest[b + 2..].find("*/") {
                        Some(e) => rest = &rest[b + 2 + e + 2..],
                        None => {
                            in_block = true;
                            break;
                        }
                    }
                }
                _ => {
                    kept += rest;
                    break;
                }
            }
        }
        out += &kept;
        out.push('\n');
    }
    out
}

/// The first keyword of a statement, upper-cased: SELECT, UPDATE, ….
pub fn command_of(sql: &str) -> String {
    strip_comments(sql).split_whitespace().next().unwrap_or("").to_ascii_uppercase()
}

/// Whether a statement only reads: safe to run under EXPLAIN ANALYZE.
pub fn reads_only(sql: &str) -> bool {
    let s = strip_comments(sql).to_ascii_lowercase();
    let first = s.split_whitespace().next().unwrap_or("");
    matches!(first, "select" | "with" | "values" | "table")
        && !["insert ", "update ", "delete ", "merge ", "truncate ", "drop ", "alter ", "create "].iter().any(|w| s.contains(w))
}

/// Find an index worth suggesting in a plan: the scan that throws most rows away.
pub fn advise(nodes: &[PlanNode], quote: impl Fn(&str) -> String) -> Option<(String, String)> {
    let worst = nodes
        .iter()
        .filter(|n| n.operation.contains("Seq Scan") && n.removed_by_filter.unwrap_or(0.) > 1000.)
        .max_by(|a, b| a.removed_by_filter.partial_cmp(&b.removed_by_filter).unwrap_or(std::cmp::Ordering::Equal))?;
    let table = worst.relation.clone()?.split_whitespace().next()?.to_string();
    let mut cols: Vec<String> = Vec::new();
    // Columns compared in the filter: an identifier followed by an operator.
    let detail = worst.detail.replace(['(', ')'], " ");
    let words: Vec<&str> = detail.split_whitespace().collect();
    for w in words.windows(2) {
        if matches!(w[1], "=" | ">" | "<" | ">=" | "<=" | "~~" | "~~*") {
            let name = w[0].rsplit('.').next().unwrap_or(w[0]).trim_matches('"').to_string();
            if name.chars().all(|c| c.is_alphanumeric() || c == '_') && !name.is_empty() && !cols.contains(&name) {
                cols.push(name);
            }
        }
    }
    if cols.is_empty() {
        return None;
    }
    let removed = worst.removed_by_filter.unwrap_or(0.);
    let kept = worst.rows.unwrap_or(0.);
    let title = format!("Seq Scan on {table} reads {} rows to keep {}.", compact(removed + kept), compact(kept));
    let sql = format!(
        "CREATE INDEX CONCURRENTLY idx_{table}_{} ON {} ({});",
        cols.join("_"),
        quote(&table),
        cols.iter().map(|c| quote(c)).collect::<Vec<_>>().join(", ")
    );
    Some((title, sql))
}

pub fn compact(n: f64) -> String {
    if n >= 1e6 {
        format!("{:.1}M", n / 1e6).replace(".0M", "M")
    } else if n >= 1e4 {
        format!("{:.0}K", n / 1e3)
    } else if n >= 1e3 {
        format!("{:.1}K", n / 1e3).replace(".0K", "K")
    } else {
        format!("{n:.0}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_on_semicolons_outside_strings() {
        let s = split_statements("select 'a;b'; -- c;\nselect $$x;y$$; select 2");
        assert_eq!(s, vec!["select 'a;b'", "-- c;\nselect $$x;y$$", "select 2"]);
    }

    #[test]
    fn finds_statement_under_cursor() {
        let sql = "select 1;\nselect 2;\n\n-- note\nselect 3";
        assert_eq!(&sql[statement_at(sql, 12).unwrap()], "select 2");
        assert_eq!(&sql[statement_at(sql, 9).unwrap()], "select 1");
        assert_eq!(&sql[statement_at(sql, 21).unwrap()], "-- note\nselect 3");
    }

    #[test]
    fn comment_only_tail_is_not_a_statement() {
        assert_eq!(split_statements("select 1; -- done").len(), 1);
    }

    #[test]
    fn writes_literal_changes() {
        let c = Change::Update { key: vec![("id".into(), Value::Int(4))], set: vec![("name".into(), Value::Text("O'Neil".into()))] };
        assert_eq!(c.to_sql(Engine::Postgres, "\"public\".\"users\""), "UPDATE \"public\".\"users\"\nSET \"name\" = 'O''Neil'\nWHERE \"id\" = 4;");
        let i = Change::Insert { values: vec![("id".into(), Value::Default), ("ok".into(), Value::Bool(true))] };
        assert_eq!(i.to_sql(Engine::Sqlite, "\"t\""), "INSERT INTO \"t\" (\"ok\")\nVALUES (TRUE);");
    }

    #[test]
    fn filters_become_where() {
        let f = vec![Filter::column("email", Op::ContainsCi, "a"), Filter::column("x", Op::IsNull, ""), Filter::column("id", Op::In, "1, 2")];
        assert_eq!(where_clause(Engine::Postgres, &f, &[]), "WHERE \"email\"::text ILIKE '%a%' AND \"x\" IS NULL AND \"id\" IN ('1', '2')");
        let any = vec![Filter { target: Target::AnyColumn, op: Op::Eq, value: "7".into() }, Filter { target: Target::Raw, op: Op::Eq, value: "a > 1".into() }];
        assert_eq!(where_clause(Engine::Sqlite, &any, &["a".into(), "b".into()]), "WHERE (\"a\" = '7' OR \"b\" = '7') AND (a > 1)");
        assert_eq!(where_clause(Engine::Sqlite, &[Filter::column("n", Op::Between, "1 and 5")], &[]), "WHERE \"n\" BETWEEN '1' AND '5'");
    }

    #[test]
    fn kinds() {
        assert_eq!(Kind::from_type("timestamp with time zone"), Kind::Time);
        assert_eq!(Kind::from_type("character varying(20)"), Kind::Text);
        assert_eq!(Kind::from_type("numeric(10,2)"), Kind::Number);
        assert_eq!(Kind::from_type("_text"), Kind::Array);
        assert_eq!(Kind::from_type("INTEGER"), Kind::Int);
    }
}
