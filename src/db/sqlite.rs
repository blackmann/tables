use super::*;
use anyhow::{Context as _, anyhow};
use rusqlite::{Connection, types::ValueRef};
use std::collections::HashMap;
use std::time::Instant;

pub struct Sqlite {
    conn: Connection,
}

impl Sqlite {
    pub fn open(path: &str) -> Result<Sqlite> {
        let path = if let Some(rest) = path.strip_prefix("~/") { format!("{}/{rest}", std::env::var("HOME").unwrap_or_default()) } else { path.to_string() };
        if path.is_empty() {
            return Err(anyhow!("Choose a database file"));
        }
        let conn = Connection::open(&path).with_context(|| format!("Opening {path}"))?;
        conn.execute_batch("PRAGMA foreign_keys = ON;")?;
        Ok(Sqlite { conn })
    }

    fn read(&self, sql: &str) -> Result<Page> {
        let start = Instant::now();
        let mut stmt = self.conn.prepare(sql)?;
        let columns: Vec<ResultColumn> = stmt
            .columns()
            .iter()
            .map(|c| ResultColumn { name: c.name().to_string(), type_name: c.decl_type().unwrap_or("").to_lowercase() })
            .collect();
        let kinds: Vec<Kind> = columns.iter().map(|c| Kind::from_type(&c.type_name)).collect();
        let n = columns.len();
        let mut rows = Vec::new();
        let mut it = stmt.query([])?;
        while let Some(r) = it.next()? {
            let mut v = Vec::with_capacity(n);
            for (i, kind) in kinds.iter().enumerate() {
                v.push(convert(r.get_ref(i)?, *kind));
            }
            rows.push(v);
        }
        Ok(Page { columns, rows, elapsed: start.elapsed() })
    }
}

fn convert(v: ValueRef, kind: Kind) -> Value {
    match v {
        ValueRef::Null => Value::Null,
        ValueRef::Integer(i) if kind == Kind::Bool => Value::Bool(i != 0),
        ValueRef::Integer(i) => Value::Int(i),
        ValueRef::Real(f) => Value::Float(f),
        ValueRef::Text(t) => {
            let s = String::from_utf8_lossy(t).into_owned();
            if kind == Kind::Json
                && let Ok(j) = serde_json::from_str(&s)
            {
                return Value::Json(j);
            }
            Value::Text(s)
        }
        ValueRef::Blob(b) => Value::Bytes(b.to_vec()),
    }
}

impl Driver for Sqlite {
    fn engine(&self) -> Engine {
        Engine::Sqlite
    }
    fn version(&mut self) -> Result<String> {
        Ok(format!("SQLite {}", rusqlite::version()))
    }
    fn schemas(&mut self) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare("PRAGMA database_list")?;
        let names = stmt.query_map([], |r| r.get::<_, String>(1))?.filter_map(|r| r.ok()).filter(|n| n != "temp").collect();
        Ok(names)
    }
    fn default_schema(&self) -> String {
        "main".into()
    }
    fn relations(&mut self, schema: &str) -> Result<Vec<Relation>> {
        let s = Engine::Sqlite.quote(schema);
        let mut stmt = self.conn.prepare(&format!("SELECT name, type FROM {s}.sqlite_master WHERE type IN ('table','view') AND name NOT LIKE 'sqlite_%' ORDER BY name"))?;
        let list: Vec<(String, String)> = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.filter_map(|r| r.ok()).collect();
        // Counting is a scan in SQLite; only do it for files that are small enough to be quick.
        let size: i64 = self
            .conn
            .query_row(&format!("SELECT page_count * page_size FROM {s}.pragma_page_count(), {s}.pragma_page_size()"), [], |r| r.get(0))
            .unwrap_or(i64::MAX);
        Ok(list
            .into_iter()
            .map(|(name, kind)| {
                let kind = if kind == "view" { RelationKind::View } else { RelationKind::Table };
                let rows = (kind == RelationKind::Table && size < 256 << 20)
                    .then(|| self.conn.query_row(&format!("SELECT count(*) FROM {s}.{}", Engine::Sqlite.quote(&name)), [], |r| r.get(0)).ok())
                    .flatten();
                Relation { name, kind, rows }
            })
            .collect())
    }
    fn functions(&mut self, _: &str) -> Result<Vec<(String, String)>> {
        Ok(vec![])
    }
    fn detail(&mut self, schema: &str, table: &str) -> Result<TableDetail> {
        let s = Engine::Sqlite.quote(schema);
        let t = quote_text(table);
        let fks: Vec<(String, String, String)> = {
            let mut stmt = self.conn.prepare(&format!("SELECT \"table\", \"from\", \"to\" FROM {s}.pragma_foreign_key_list({t})"))?;
            stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get::<_, Option<String>>(2)?.unwrap_or_else(|| "rowid".into()))))?.filter_map(|r| r.ok()).collect()
        };
        let indexes: Vec<(String, bool, String)> = {
            let mut stmt = self.conn.prepare(&format!("SELECT name, \"unique\", origin FROM {s}.pragma_index_list({t})"))?;
            stmt.query_map([], |r| Ok((r.get(0)?, r.get::<_, i64>(1)? != 0, r.get(2)?)))?.filter_map(|r| r.ok()).collect()
        };
        let mut index_info = Vec::new();
        let mut unique_cols = Vec::new();
        for (name, unique, origin) in &indexes {
            let mut stmt = self.conn.prepare(&format!("SELECT name FROM {s}.pragma_index_info({})", quote_text(name)))?;
            let cols: Vec<String> = stmt.query_map([], |r| r.get::<_, Option<String>>(0))?.filter_map(|r| r.ok().flatten()).collect();
            if *unique && cols.len() == 1 {
                unique_cols.push(cols[0].clone());
            }
            index_info.push(IndexInfo { name: name.clone(), columns: cols, unique: *unique, primary: origin == "pk", method: "btree".into(), size: None, scans: None });
        }
        let columns: Vec<ColumnInfo> = {
            let mut stmt = self.conn.prepare(&format!("SELECT name, type, \"notnull\", dflt_value, pk FROM {s}.pragma_table_xinfo({t}) WHERE hidden = 0"))?;
            stmt.query_map([], |r| {
                let name: String = r.get(0)?;
                Ok(ColumnInfo {
                    foreign: fks.iter().find(|f| f.1 == name).map(|f| (f.0.clone(), f.2.clone())),
                    unique: unique_cols.contains(&name),
                    data_type: r.get::<_, String>(1)?.to_lowercase(),
                    nullable: r.get::<_, i64>(2)? == 0,
                    default: r.get(3)?,
                    primary_key: r.get::<_, i64>(4)? > 0,
                    enum_values: vec![],
                    comment: None,
                    name,
                })
            })?
            .filter_map(|r| r.ok())
            .collect()
        };
        let ddl: String = self
            .conn
            .query_row(&format!("SELECT sql FROM {s}.sqlite_master WHERE name = {t}"), [], |r| r.get::<_, Option<String>>(0))
            .ok()
            .flatten()
            .unwrap_or_default();
        let mut referenced_by = Vec::new();
        let others: Vec<String> = {
            let mut stmt = self.conn.prepare(&format!("SELECT name FROM {s}.sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'"))?;
            stmt.query_map([], |r| r.get(0))?.filter_map(|r| r.ok()).collect()
        };
        for other in others {
            let mut stmt = self.conn.prepare(&format!("SELECT \"table\", \"from\" FROM {s}.pragma_foreign_key_list({})", quote_text(&other)))?;
            for (target, from) in stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?.filter_map(|r| r.ok()) {
                if target.eq_ignore_ascii_case(table) {
                    referenced_by.push((other.clone(), from));
                }
            }
        }
        Ok(TableDetail { columns, indexes: index_info, referenced_by, ddl: format!("{};", ddl.trim_end_matches(';')), size: None })
    }
    fn all_columns(&mut self, schema: &str) -> Result<Vec<(String, String, String)>> {
        let s = Engine::Sqlite.quote(schema);
        let mut stmt = self.conn.prepare(&format!(
            "SELECT m.name, p.name, lower(p.type) FROM {s}.sqlite_master m, pragma_table_info(m.name) p WHERE m.type IN ('table','view') AND m.name NOT LIKE 'sqlite_%' ORDER BY m.name, p.cid"
        ))?;
        Ok(stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.filter_map(|r| r.ok()).collect())
    }
    fn query(&mut self, sql: &str) -> Result<Page> {
        self.read(sql)
    }
    fn execute(&mut self, sql: &str) -> Result<Vec<(String, Outcome)>, (Vec<(String, Outcome)>, String, anyhow::Error)> {
        let mut done = Vec::new();
        for stmt_sql in split_statements(sql) {
            let start = Instant::now();
            let res: Result<Outcome> = (|| {
                let stmt = self.conn.prepare(&stmt_sql)?;
                if stmt.column_count() > 0 {
                    drop(stmt);
                    return Ok(Outcome::Rows(self.read(&stmt_sql)?));
                }
                drop(stmt);
                let n = self.conn.execute(&stmt_sql, [])?;
                let cmd = command_of(&stmt_sql);
                let rows = matches!(cmd.as_str(), "INSERT" | "UPDATE" | "DELETE" | "REPLACE").then_some(n as u64);
                Ok(Outcome::Done { command: cmd, rows, elapsed: start.elapsed() })
            })();
            match res {
                Ok(o) => done.push((stmt_sql, o)),
                Err(e) => return Err((done, stmt_sql, e)),
            }
        }
        Ok(done)
    }
    fn apply(&mut self, statements: &[String]) -> Result<()> {
        let tx = self.conn.transaction()?;
        for s in statements {
            tx.execute_batch(s).map_err(|e| anyhow!("{e}\n\n{s}"))?;
        }
        tx.commit()?;
        Ok(())
    }
    fn explain(&mut self, sql: &str) -> Result<Plan> {
        let mut stmt = self.conn.prepare(&format!("EXPLAIN QUERY PLAN {}", sql.trim().trim_end_matches(';')))?;
        let rows: Vec<(i64, i64, String)> = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(3)?)))?.filter_map(|r| r.ok()).collect();
        let mut depth: HashMap<i64, usize> = HashMap::new();
        let nodes = rows
            .into_iter()
            .map(|(id, parent, detail)| {
                let d = depth.get(&parent).map_or(0, |d| d + 1);
                depth.insert(id, d);
                let mut words = detail.splitn(3, ' ');
                let op = words.next().unwrap_or("").to_string();
                let (relation, rest) = if matches!(op.as_str(), "SCAN" | "SEARCH") {
                    (words.next().map(String::from), words.next().unwrap_or("").to_string())
                } else {
                    (None, detail[op.len()..].trim().to_string())
                };
                let op = match op.as_str() {
                    "SCAN" if !rest.contains("INDEX") => "Seq Scan".to_string(),
                    "SCAN" => "Index Scan".to_string(),
                    "SEARCH" => "Index Search".to_string(),
                    _ => detail.clone(),
                };
                let rest = if op == detail { String::new() } else { rest };
                PlanNode { depth: d, operation: op, relation, detail: rest, ..Default::default() }
            })
            .collect();
        Ok(Plan { nodes, summary: "EXPLAIN QUERY PLAN — SQLite doesn't report timings".into(), advice: None })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reads_and_writes() {
        let mut db = Sqlite { conn: Connection::open_in_memory().unwrap() };
        let r = db.execute("create table a(id integer primary key, ok boolean, j json); insert into a(ok, j) values (1, '{\"x\":1}'); select * from a").unwrap();
        assert_eq!(r.len(), 3);
        let Outcome::Rows(p) = &r[2].1 else { panic!() };
        assert_eq!(p.rows[0], vec![Value::Int(1), Value::Bool(true), Value::Json(serde_json::json!({"x":1}))]);
        let d = db.detail("main", "a").unwrap();
        assert!(d.columns[0].primary_key);
        assert!(db.apply(&["update a set ok = 0".into(), "nonsense".into()]).is_err());
        assert_eq!(db.query("select ok from a").unwrap().rows[0][0], Value::Bool(true));
    }
}
